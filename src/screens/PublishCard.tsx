import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { useT, type Key } from "@/i18n";
import { asUiError } from "@/lib/api";
import { realmRegistry, useRegistryStatus } from "@/lib/registry";
import { asControlError, control, type ControlStatus } from "@/lib/control";

const LANGUAGES = ["en", "ru", "de", "fr", "es", "zh", "pt", "pl", "uk"];

/** Publishing this server's realm to the central Registry: a name, a description, a language. Nothing else leaves this machine. */
export function PublishCard({ serverId, serverName }: { serverId: string; serverName: string }) {
  const t = useT();
  const localId = `srv-${serverId}`;
  const { status, refresh } = useRegistryStatus();
  const mine = status?.realms.find((r) => r.local_id === localId);
  const published = !!mine?.enabled;
  const [url, setUrl] = useState("");
  const [name, setName] = useState(serverName);
  const [description, setDescription] = useState("");
  const [language, setLanguage] = useState("en");
  const [region, setRegion] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [existingOnly, setExistingOnly] = useState(false);
  const [route, setRoute] = useState("");
  const [accessSaved, setAccessSaved] = useState(false);
  const [ctl, setCtl] = useState<ControlStatus | null>(null);
  const link = ctl?.hosting.find((h) => h.local_id === localId)?.link;

  useEffect(() => {
    const load = () => void control.status().then(setCtl).catch(() => {});
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => { if (mine) { setExistingOnly(mine.existing_only); setRoute(mine.route ?? ""); } }, [mine?.existing_only, mine?.route]);

  async function saveAccess() {
    setBusy(true); setError(null); setAccessSaved(false);
    try { await control.setAccess(localId, existingOnly, route.trim() === "" ? null : route.trim()); await refresh(); setAccessSaved(true); } catch (e) { setError(asControlError(e).message); } finally { setBusy(false); }
  }

  useEffect(() => { if (status && status.url !== null) setUrl((u) => (u === "" ? status.url ?? "" : u)); }, [status?.url]);
  useEffect(() => {
    if (mine && mine.display_name) { setName(mine.display_name); setDescription(mine.description); setLanguage(mine.language || "en"); setRegion(mine.region ?? ""); }
  }, [mine?.display_name, mine?.description, mine?.language]);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try { await action(); await refresh(); } catch (e) { setError(asUiError(e).technical); } finally { setBusy(false); }
  }

  const tone = mine?.state === "online" ? "text-ok" : mine?.state === "rejected" ? "text-bad" : mine?.state === "retrying" || mine?.state === "realm_stopped" || mine?.state === "waiting_for_realm" ? "text-warn" : "text-muted";

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">{t("registry.title")}</h2>
      <p className="mt-1 text-sm text-muted">{t("registry.text")}</p>

      <label htmlFor="registry-url" className="mt-4 block text-sm font-medium">{t("registry.url")}</label>
      <div className="mt-1 flex gap-2">
        <input id="registry-url" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://coa-manager.duckdns.org" className="min-w-0 flex-1 rounded-md border border-line bg-card px-3 py-2" />
        <Button size="sm" disabled={busy || url.trim() === (status?.url ?? "")} onClick={() => void run(() => realmRegistry.setUrl(url.trim() || null))}>{t("registry.urlSave")}</Button>
      </div>

      {!published && <>
        <label htmlFor="registry-name" className="mt-4 block text-sm font-medium">{t("registry.name")}</label>
        <input id="registry-name" value={name} maxLength={80} onChange={(e) => setName(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
        <label htmlFor="registry-description" className="mt-3 block text-sm font-medium">{t("registry.description")}</label>
        <textarea id="registry-description" value={description} rows={3} onChange={(e) => setDescription(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
        <p className="mt-1 text-xs text-muted">{t("registry.descriptionHint")}</p>
        <label htmlFor="registry-language" className="mt-3 block text-sm font-medium">{t("registry.language")}</label>
        <select id="registry-language" value={language} onChange={(e) => setLanguage(e.target.value)} className="mt-1 rounded-md border border-line bg-card px-3 py-2">
          {LANGUAGES.map((l) => <option key={l} value={l}>{l}</option>)}
        </select>
        <label htmlFor="registry-region" className="mt-3 block text-sm font-medium">{t("registry.region")}</label>
        <input id="registry-region" value={region} maxLength={16} placeholder="EU" onChange={(e) => setRegion(e.target.value)} className="mt-1 w-32 rounded-md border border-line bg-card px-3 py-2" />
        <div className="mt-4">
          <Button variant="primary" disabled={busy || !status?.url || name.trim() === ""} onClick={() => void run(() => realmRegistry.publish(localId, name, description, language, region.trim() === "" ? null : region.trim()))}>
            {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("registry.publish")}
          </Button>
          {!status?.url && <p className="mt-2 text-xs text-muted">{t("registry.noUrl")}</p>}
        </div>
      </>}

      {mine && (published || mine.state === "unpublishing") && <div className="mt-4 rounded-md border border-line p-3 text-sm" role="status">
        <p className={cn("font-medium", tone)} data-state={mine.state}>{t(`registry.state.${mine.state}` as Key)}</p>
        <p className="mt-1 text-xs text-muted">{mine.display_name}{mine.metadata_revision ? ` · ${t("registry.revision", { n: mine.metadata_revision })}` : ""}</p>
        {mine.last_error && <p className="mt-1 text-xs text-warn">{mine.last_error}</p>}
        {mine.state === "retrying" && mine.retry_in_secs !== null && <p className="mt-1 text-xs text-muted">{t("registry.retryIn", { s: mine.retry_in_secs })}</p>}
        <div className="mt-3 flex gap-2">
          {mine.state === "rejected" && <Button size="sm" onClick={() => void run(async () => { await realmRegistry.retry(localId); })}>{t("registry.retry")}</Button>}
          <Button size="sm" disabled={busy} onClick={() => void run(() => realmRegistry.unpublish(localId))}>{t("registry.stop")}</Button>
        </div>
      </div>}
      {mine && (published || mine.state === "unpublishing") && <div className="mt-4 rounded-md border border-line p-3 text-sm" data-testid="access-options">
        <p className="font-medium">{t("access.title")}</p>
        <p className="mt-1 text-xs text-muted">{t("access.text")}</p>
        <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={existingOnly} onChange={(e) => setExistingOnly(e.target.checked)} className="mt-1" data-testid="existing-only" /><span>{t("access.existingOnly")}<span className="block text-xs text-muted">{t("access.existingOnlyHint")}</span></span></label>
        <label htmlFor="access-route" className="mt-3 block font-medium">{t("access.route")}</label>
        <input id="access-route" value={route} placeholder="play.example.org:3724" onChange={(e) => setRoute(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" data-testid="access-route" />
        <p className="mt-1 text-xs text-muted">{t("access.routeHint")}</p>
        <div className="mt-3 flex items-center gap-3">
          <Button size="sm" disabled={busy} onClick={() => void saveAccess()}>{t("access.save")}</Button>
          {accessSaved && <span className="text-xs text-ok" role="status">{t("access.saved")}</span>}
        </div>
        <p className={cn("mt-3 text-xs", link?.connected ? "text-ok" : "text-muted")} data-testid="control-link" data-connected={link?.connected ? "yes" : "no"}>
          {link?.connected ? t("access.linkUp") : t("access.linkDown")}{link?.last_error && !link.connected ? ` — ${link.last_error}` : ""}
        </p>
        {ctl && ctl.secret_store === "file" && <p className="mt-1 text-xs text-warn">{t("access.fileStore")}</p>}
      </div>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
    </Card>
  );
}
