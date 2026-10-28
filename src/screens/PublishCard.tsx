import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { useT, type Key } from "@/i18n";
import { api, asUiError, type RealmMode } from "@/lib/api";
import { realmRegistry, useRegistryStatus } from "@/lib/registry";
import { asControlError, control, type ControlStatus } from "@/lib/control";

const LANGUAGES = ["en", "ru", "de", "fr", "es", "zh", "pt", "pl", "uk"];

const REALM_LABEL: Record<RealmMode, string> = { coa: "Conquest of Azeroth", wildcard: "Wildcard", reborn: "Warcraft Reborn" };

/** The realms this server runs: the main one keeps the plain id it was published under, each realm that starts next to it has its own. */
export function PublishCard({ serverId, serverName }: { serverId: string; serverName: string }) {
  const [realms, setRealms] = useState<{ localId: string; mode: RealmMode }[] | null>(null);
  useEffect(() => {
    let alive = true;
    void api.realmProfiles(serverId).then((r) => {
      if (!alive) return;
      const extra: RealmMode[] = [];
      if (r.simultaneous) extra.push(r.active === "coa" ? "wildcard" : "coa");
      if (r.with_reborn) extra.push("reborn");
      setRealms([{ localId: `srv-${serverId}`, mode: r.active }, ...extra.map((mode) => ({ localId: `srv-${serverId}:${mode}`, mode }))]);
    }).catch(() => { if (alive) setRealms([{ localId: `srv-${serverId}`, mode: "coa" }]); });
    return () => { alive = false; };
  }, [serverId]);
  if (!realms) return null;
  const several = realms.length > 1;
  return <>{realms.map((r) => (
    <RealmPublish key={r.localId} localId={r.localId} serverName={several && r.mode !== realms[0].mode ? `${serverName} · ${REALM_LABEL[r.mode]}` : serverName} realmLabel={several ? REALM_LABEL[r.mode] : null} />
  ))}</>;
}

/** Publishing one realm to the central Registry: a name, a description, a language. Nothing else leaves this machine. */
function RealmPublish({ localId, serverName, realmLabel }: { localId: string; serverName: string; realmLabel: string | null }) {
  const t = useT();
  const { status, refresh } = useRegistryStatus();
  const mine = status?.realms.find((r) => r.local_id === localId);
  const published = !!mine?.enabled;
  const [name, setName] = useState(serverName);
  const [description, setDescription] = useState("");
  const [language, setLanguage] = useState("en");
  const [region, setRegion] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [allowPortable, setAllowPortable] = useState(false);
  const [existingOnly, setExistingOnly] = useState(false);
  const [route, setRoute] = useState("");
  const [accessSaved, setAccessSaved] = useState(false);
  const [ctl, setCtl] = useState<ControlStatus | null>(null);
  const host = ctl?.hosting.find((h) => h.local_id === localId);
  const link = host?.link;

  useEffect(() => {
    const load = () => void control.status().then(setCtl).catch(() => {});
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => { if (mine) { setExistingOnly(mine.existing_only); setAllowPortable(mine.allow_portable); setRoute(mine.route ?? ""); } }, [mine?.existing_only, mine?.allow_portable, mine?.route]);

  async function saveAccess() {
    setBusy(true); setError(null); setAccessSaved(false);
    try { await control.setAccess(localId, existingOnly, allowPortable, route.trim() === "" ? null : route.trim()); await refresh(); setAccessSaved(true); } catch (e) { setError(asControlError(e).message); } finally { setBusy(false); }
  }

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
      <h2 className="font-semibold">{t("registry.title")}{realmLabel ? ` · ${realmLabel}` : ""}</h2>
      <p className="mt-1 text-sm text-muted">{t("registry.text")}</p>


      {!published && <>
        <label htmlFor={`registry-name-${localId}`} className="mt-4 block text-sm font-medium">{t("registry.name")}</label>
        <input id={`registry-name-${localId}`} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
        <label htmlFor={`registry-description-${localId}`} className="mt-3 block text-sm font-medium">{t("registry.description")}</label>
        <textarea id={`registry-description-${localId}`} value={description} rows={3} onChange={(e) => setDescription(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
        <p className="mt-1 text-xs text-muted">{t("registry.descriptionHint")}</p>
        <label htmlFor={`registry-language-${localId}`} className="mt-3 block text-sm font-medium">{t("registry.language")}</label>
        <select id={`registry-language-${localId}`} value={language} onChange={(e) => setLanguage(e.target.value)} className="mt-1 rounded-md border border-line bg-card px-3 py-2">
          {LANGUAGES.map((l) => <option key={l} value={l}>{l}</option>)}
        </select>
        <label htmlFor={`registry-region-${localId}`} className="mt-3 block text-sm font-medium">{t("registry.region")}</label>
        <input id={`registry-region-${localId}`} value={region} maxLength={16} placeholder="EU" onChange={(e) => setRegion(e.target.value)} className="mt-1 w-32 rounded-md border border-line bg-card px-3 py-2" />
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
        <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={allowPortable} onChange={(e) => setAllowPortable(e.target.checked)} className="mt-1" data-testid="allow-portable" /><span>{t("access.allowPortable")}<span className="block text-xs text-muted">{t("access.allowPortableHint")}</span></span></label>
        <label htmlFor={`access-route-${localId}`} className="mt-3 block font-medium">{t("access.route")}</label>
        <input id={`access-route-${localId}`} value={route} placeholder="play.example.org:3724" onChange={(e) => setRoute(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" data-testid={`access-route-${localId}`} />
        <p className="mt-1 text-xs text-muted">{t("access.routeHint")}</p>
        <div className="mt-3 flex items-center gap-3">
          <Button size="sm" disabled={busy} onClick={() => void saveAccess()}>{t("access.save")}</Button>
          {accessSaved && <span className="text-xs text-ok" role="status">{t("access.saved")}</span>}
        </div>
        <p className={cn("mt-3 text-xs", link?.connected ? "text-ok" : "text-muted")} data-testid="control-link" data-connected={link?.connected ? "yes" : "no"}>
          {link?.connected ? t("access.linkUp") : t("access.linkDown")}{link?.last_error && !link.connected ? ` — ${link.last_error}` : ""}
        </p>
        <p className={cn("mt-2 text-xs", host?.relay?.connected ? "text-ok" : "text-warn")} data-testid="relay-link">{host?.relay?.connected ? t("access.relayUp") : t("access.relayDown")}{host?.relay?.last_error ? ` — ${host.relay.last_error}` : ""}</p>
        {ctl && ctl.secret_store === "file" && <p className="mt-1 text-xs text-warn">{t("access.fileStore")}</p>}
      </div>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
    </Card>
  );
}
