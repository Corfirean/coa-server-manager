import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { useHuman, useI18n } from "@/i18n";
import { api, asUiError, type PbcConnection, type PbcConnectionSetup, type UiError } from "@/lib/api";
import type { UnsavedChangesGuard } from "@/lib/unsavedGuard";

type Provider = "openai" | "ollama" | "anthropic" | "compatible";
const presets: Record<Provider, PbcConnection> = {
  openai: { apiType: "openai", baseUrl: "https://api.openai.com/v1", model: "gpt-5.4-mini" },
  ollama: { apiType: "ollama", baseUrl: "http://127.0.0.1:11434", model: "" },
  anthropic: { apiType: "anthropic", baseUrl: "https://api.anthropic.com/v1", model: "" },
  compatible: { apiType: "openai", baseUrl: "", model: "" },
};
const providerOf = (connection: PbcConnection): Provider => connection.apiType !== "openai" ? connection.apiType
  : connection.baseUrl === presets.openai.baseUrl ? "openai" : "compatible";
const inputClass = "selectable w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold";

export function PbcConnectionSettings({ serverId, onSaved, onRegisterGuard }: {
  serverId: string;
  onSaved: () => void;
  onRegisterGuard: (guard: UnsavedChangesGuard | null) => void;
}) {
  const { t } = useI18n();
  const human = useHuman();
  const [setup, setSetup] = useState<PbcConnectionSetup | null>(null);
  const [connection, setConnection] = useState<PbcConnection>(presets.openai);
  const [provider, setProvider] = useState<Provider>("openai");
  const [apiKey, setApiKey] = useState("");
  const [removeKey, setRemoveKey] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [saved, setSaved] = useState(false);

  const reset = useCallback((value: PbcConnectionSetup) => {
    const current = value.connection ?? presets.openai;
    setConnection(current);
    setProvider(providerOf(current));
    setApiKey("");
    setRemoveKey(false);
  }, []);

  useEffect(() => {
    let cancelled = false;
    setSetup(null);
    setSaved(false);
    setError(null);
    void api.pbcConnection(serverId).then(value => {
      if (cancelled) return;
      setSetup(value);
      reset(value);
    }).catch(e => { if (!cancelled) setError(asUiError(e)); });
    return () => { cancelled = true; };
  }, [serverId, reset]);

  const previous = setup?.connection ?? presets.openai;
  const dirty = !!setup && (connection.apiType !== previous.apiType || connection.baseUrl !== previous.baseUrl
    || connection.model !== previous.model || !!apiKey || removeKey);
  const sameEndpoint = connection.apiType === previous.apiType
    && connection.baseUrl.trim().replace(/\/+$/, "") === previous.baseUrl;
  const hasKey = !!setup?.connection?.hasKey && sameEndpoint;
  const valid = !!connection.baseUrl.trim() && !!connection.model.trim();

  const save = useCallback(async () => {
    if (!setup || !valid || busy) return false;
    setBusy(true);
    setSaved(false);
    setError(null);
    try {
      const next = await api.pbcSaveConnection(serverId, { ...connection, apiKey: apiKey || undefined, removeKey });
      setSetup(next);
      reset(next);
      setSaved(true);
      onSaved();
      return true;
    } catch (e) {
      setError(asUiError(e));
      return false;
    } finally {
      setBusy(false);
    }
  }, [setup, valid, busy, serverId, connection, apiKey, removeKey, reset, onSaved]);

  useEffect(() => {
    if (!setup || !dirty) return;
    onRegisterGuard({ isDirty: () => true, save, discard: () => reset(setup) });
    return () => onRegisterGuard(null);
  }, [setup, dirty, save, reset, onRegisterGuard]);

  function changeProvider(value: Provider) {
    setProvider(value);
    setConnection(presets[value]);
    setApiKey("");
    setRemoveKey(false);
    setSaved(false);
  }

  return (
    <section className="mt-4 space-y-3 border-t border-line pt-4" aria-labelledby="pbc-connection-heading">
      <h3 id="pbc-connection-heading" className="font-semibold">{t("mod.pbc.connection")}</h3>
      {error && <p className="text-sm text-warn" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
      {setup && (
        <form className="space-y-3" onSubmit={event => { event.preventDefault(); void save(); }}>
          {!setup.active && <p className="text-sm text-muted">{t("mod.pbc.useConnection")}</p>}
          <fieldset disabled={busy} className="grid gap-3 sm:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span>{t("mod.pbc.provider")}</span>
              <select className={inputClass} value={provider} onChange={event => changeProvider(event.target.value as Provider)}>
                <option value="openai">OpenAI</option>
                <option value="ollama">Ollama</option>
                <option value="anthropic">Anthropic</option>
                <option value="compatible">{t("mod.pbc.compatible")}</option>
              </select>
            </label>
            <label className="space-y-1 text-sm">
              <span>{t("mod.pbc.model")}</span>
              <input className={inputClass} value={connection.model} required onChange={event => { setConnection({ ...connection, model: event.target.value }); setSaved(false); }} />
            </label>
            <label className="space-y-1 text-sm sm:col-span-2">
              <span>{t("mod.pbc.endpoint")}</span>
              <input className={inputClass} value={connection.baseUrl} type="url" required readOnly={provider === "openai"}
                onChange={event => { setConnection({ ...connection, baseUrl: event.target.value }); setApiKey(""); setRemoveKey(false); setSaved(false); }} />
            </label>
            <label className="space-y-1 text-sm sm:col-span-2">
              <span>{t("mod.pbc.key")}</span>
              <input className={inputClass} value={apiKey} type="password" autoComplete="off" disabled={removeKey}
                placeholder={hasKey ? t("mod.pbc.savedKey") : undefined}
                onChange={event => { setApiKey(event.target.value); setSaved(false); }} />
            </label>
            {hasKey && <label className="flex items-center gap-2 text-sm sm:col-span-2">
              <input type="checkbox" checked={removeKey} onChange={event => { setRemoveKey(event.target.checked); setApiKey(""); setSaved(false); }} />
              {t("mod.pbc.removeKey")}
            </label>}
          </fieldset>
          <p className="text-sm text-muted">{t("mod.pbc.keyHint")}</p>
          <Button size="sm" type="submit" disabled={busy || !valid || (setup.active && !dirty)}>{t("mod.pbc.save")}</Button>
          {saved && <p className="text-sm text-ok" role="status">{t("mod.pbc.saved")}</p>}
        </form>
      )}
    </section>
  );
}
