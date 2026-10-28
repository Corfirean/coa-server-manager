import { useEffect, useState, type ReactNode } from "react";
import { Loader2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { useT, type Key } from "@/i18n";
import { asUiError, api } from "@/lib/api";
import { realmRegistry, useRegistryStatus } from "@/lib/registry";
import { asControlError, control, type ControlStatus } from "@/lib/control";
import { isListed, stateTone, type PublishRealm } from "@/lib/publishRealms";

const LANGUAGES = ["en", "ru", "de", "fr", "es", "zh", "pt", "pl", "uk"];

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const t = useT();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 md:p-6" role="presentation" onClick={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <Card className="flex max-h-[88vh] w-full max-w-xl flex-col p-6 shadow-2xl" role="dialog" aria-modal="true" aria-label={title}>
        <div className="flex items-center justify-between border-b border-line pb-4">
          <h2 className="text-lg font-semibold">{title}</h2>
          <button type="button" onClick={onClose} className="cursor-pointer text-muted transition-colors hover:text-ink" aria-label={t("btn.close")}>
            <X className="h-5 w-5" aria-hidden />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto pt-4">{children}</div>
      </Card>
    </div>
  );
}

/** The name, description, language and region of a listing; the fields are the same for one realm and for all of them. */
function ListingFields({ idPrefix, name, setName, description, setDescription, language, setLanguage, region, setRegion, hideName }: {
  idPrefix: string; name: string; setName: (v: string) => void; description: string; setDescription: (v: string) => void;
  language: string; setLanguage: (v: string) => void; region: string; setRegion: (v: string) => void; hideName?: boolean;
}) {
  const t = useT();
  return <>
    {!hideName && <>
      <label htmlFor={`${idPrefix}-name`} className="block text-sm font-medium">{t("registry.name")}</label>
      <input id={`${idPrefix}-name`} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
    </>}
    <label htmlFor={`${idPrefix}-description`} className="mt-3 block text-sm font-medium">{t("registry.description")}</label>
    <textarea id={`${idPrefix}-description`} value={description} rows={3} onChange={(e) => setDescription(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
    <p className="mt-1 text-xs text-muted">{t("registry.descriptionHint")}</p>
    <div className="mt-3 flex flex-wrap gap-4">
      <div>
        <label htmlFor={`${idPrefix}-language`} className="block text-sm font-medium">{t("registry.language")}</label>
        <select id={`${idPrefix}-language`} value={language} onChange={(e) => setLanguage(e.target.value)} className="mt-1 rounded-md border border-line bg-card px-3 py-2">
          {LANGUAGES.map((l) => <option key={l} value={l}>{l}</option>)}
        </select>
      </div>
      <div>
        <label htmlFor={`${idPrefix}-region`} className="block text-sm font-medium">{t("registry.region")}</label>
        <input id={`${idPrefix}-region`} value={region} maxLength={16} placeholder="EU" onChange={(e) => setRegion(e.target.value)} className="mt-1 w-32 rounded-md border border-line bg-card px-3 py-2" />
      </div>
    </div>
  </>;
}

/** Who may join and how: shown before publishing, so the choice is made once, not found later. */
function JoinChoices({ existingOnly, setExistingOnly, allowPortable, setAllowPortable }: { existingOnly: boolean; setExistingOnly: (v: boolean) => void; allowPortable: boolean; setAllowPortable: (v: boolean) => void }) {
  const t = useT();
  return (
    <div className="mt-4 rounded-md border border-line p-3 text-sm">
      <p className="font-medium">{t("access.title")}</p>
      <p className="mt-1 text-xs text-muted">{t("access.text")}</p>
      <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={existingOnly} onChange={(e) => setExistingOnly(e.target.checked)} className="mt-1" /><span>{t("access.existingOnly")}<span className="block text-xs text-muted">{t("access.existingOnlyHint")}</span></span></label>
      <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={allowPortable} onChange={(e) => setAllowPortable(e.target.checked)} className="mt-1" /><span>{t("access.allowPortable")}<span className="block text-xs text-muted">{t("access.allowPortableHint")}</span></span></label>
    </div>
  );
}

/** Publishing, stopping and the access options of one realm. */
export function PublishRealmDialog({ realm, onClose }: { realm: PublishRealm; onClose: () => void }) {
  const t = useT();
  const { localId } = realm;
  const { status, refresh } = useRegistryStatus();
  const mine = status?.realms.find((r) => r.local_id === localId);
  const published = !!mine?.enabled;
  const [name, setName] = useState(realm.defaultName);
  const [description, setDescription] = useState("");
  const [language, setLanguage] = useState("en");
  const [region, setRegion] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // New listings start with portable characters allowed and accounts created for new players; the choice is shown before publishing.
  const [allowPortable, setAllowPortable] = useState(true);
  const [existingOnly, setExistingOnly] = useState(false);
  const [route, setRoute] = useState("");
  const [accessSaved, setAccessSaved] = useState(false);
  const [portableEnabled, setPortableEnabled] = useState<boolean | null>(null);
  const primaryRealm = !localId.includes(":");
  const [ctl, setCtl] = useState<ControlStatus | null>(null);
  const host = ctl?.hosting.find((h) => h.local_id === localId);
  const link = host?.link;

  useEffect(() => {
    if (!primaryRealm) return;
    let active = true;
    void api.allSettings(localId.slice(4)).then((items) => {
      const item = items.find((i) => i.key === "PortableSession.Enable");
      if (active && item) setPortableEnabled(item.value === "1" || item.value.toLowerCase() === "true");
    }).catch((e) => { if (active) setError(asUiError(e).technical); });
    return () => { active = false; };
  }, [localId, primaryRealm]);

  useEffect(() => {
    const load = () => void control.status().then(setCtl).catch(() => {});
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => { if (mine) { setExistingOnly(mine.existing_only); setAllowPortable(mine.allow_portable); setRoute(mine.route ?? ""); } }, [mine?.existing_only, mine?.allow_portable, mine?.route]);
  useEffect(() => {
    if (mine && mine.display_name) { setName(mine.display_name); setDescription(mine.description); setLanguage(mine.language || "en"); setRegion(mine.region ?? ""); }
  }, [mine?.display_name, mine?.description, mine?.language]);

  async function saveAccess() {
    setBusy(true); setError(null); setAccessSaved(false);
    try {
      if (primaryRealm && portableEnabled !== null) {
        await api.allSettingsSave(localId.slice(4), {
          "PortableSession.Enable": portableEnabled ? "1" : "0",
          ...(portableEnabled ? { "CharacterDatabase.WorkerThreads": "1" } : {}),
        });
      }
      await control.setAccess(localId, existingOnly, allowPortable, route.trim() === "" ? null : route.trim());
      await refresh();
      setAccessSaved(true);
    } catch (e) { setError(asControlError(e).message); } finally { setBusy(false); }
  }
  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try { await action(); await refresh(); } catch (e) { setError(asUiError(e).technical); } finally { setBusy(false); }
  }
  async function publish() {
    setBusy(true);
    setError(null);
    try {
      await realmRegistry.publish(localId, name, description, language, region.trim() === "" ? null : region.trim());
      // Saving the joining options needs the listing to exist; if it cannot be saved now (for example the server is running), the options stay available below.
      try { await control.setAccess(localId, existingOnly, allowPortable, route.trim() === "" ? null : route.trim()); }
      catch (e) { setError(t("registry.accessFailed", { reason: asControlError(e).message })); }
      await refresh();
    } catch (e) { setError(asUiError(e).technical); } finally { setBusy(false); }
  }

  return (
    <Modal title={t("registry.dialogTitle", { realm: realm.label })} onClose={onClose}>
      <p className="text-sm text-muted">{t("registry.text")}</p>

      {!published && <div className="mt-4">
        <ListingFields idPrefix={`pub-${localId}`} name={name} setName={setName} description={description} setDescription={setDescription} language={language} setLanguage={setLanguage} region={region} setRegion={setRegion} />
        <JoinChoices existingOnly={existingOnly} setExistingOnly={setExistingOnly} allowPortable={allowPortable} setAllowPortable={setAllowPortable} />
        <div className="mt-4">
          <Button variant="primary" disabled={busy || !status?.url || name.trim() === ""} onClick={() => void publish()}>
            {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("registry.publish")}
          </Button>
          {!status?.url && <p className="mt-2 text-xs text-muted">{t("registry.noUrl")}</p>}
        </div>
      </div>}

      {mine && isListed(mine) && <div className="mt-4 rounded-md border border-line p-3 text-sm" role="status">
        <p className={cn("font-medium", stateTone(mine.state))} data-state={mine.state}>{t(`registry.state.${mine.state}` as Key)}</p>
        {mine.state === "realm_stopped" && <p className="mt-2 text-xs text-warn">{t("registry.realmStoppedHint")}</p>}
        <p className="mt-1 text-xs text-muted">{mine.display_name}{mine.metadata_revision ? ` · ${t("registry.revision", { n: mine.metadata_revision })}` : ""}</p>
        {mine.last_error && <p className="mt-1 text-xs text-warn">{mine.last_error}</p>}
        {mine.state === "retrying" && mine.retry_in_secs !== null && <p className="mt-1 text-xs text-muted">{t("registry.retryIn", { s: mine.retry_in_secs })}</p>}
        <div className="mt-3 flex gap-2">
          {mine.state === "rejected" && <Button size="sm" onClick={() => void run(async () => { await realmRegistry.retry(localId); })}>{t("registry.retry")}</Button>}
          <Button size="sm" disabled={busy} onClick={() => void run(() => realmRegistry.unpublish(localId))}>{t("registry.stop")}</Button>
        </div>
      </div>}

      {mine && isListed(mine) && <div className="mt-4 rounded-md border border-line p-3 text-sm" data-testid="access-options">
        <p className="font-medium">{t("access.title")}</p>
        <p className="mt-1 text-xs text-muted">{t("access.text")}</p>
        {primaryRealm && <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={portableEnabled ?? false} disabled={portableEnabled === null} onChange={(e) => setPortableEnabled(e.target.checked)} className="mt-1" data-testid="portable-enabled" /><span>{t("access.portableEnabled")}<span className="block text-xs text-muted">{t("access.portableEnabledHint")}</span></span></label>}
        <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={existingOnly} onChange={(e) => setExistingOnly(e.target.checked)} className="mt-1" data-testid="existing-only" /><span>{t("access.existingOnly")}<span className="block text-xs text-muted">{t("access.existingOnlyHint")}</span></span></label>
        <label className="mt-3 flex items-start gap-2"><input type="checkbox" checked={allowPortable} onChange={(e) => setAllowPortable(e.target.checked)} className="mt-1" data-testid="allow-portable" /><span>{t("access.allowPortable")}<span className="block text-xs text-muted">{t("access.allowPortableHint")}</span></span></label>
        <label htmlFor={`access-route-${localId}`} className="mt-3 block font-medium">{t("access.route")}</label>
        <input id={`access-route-${localId}`} value={route} placeholder="play.example.org:3724" onChange={(e) => setRoute(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" data-testid="access-route" />
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
    </Modal>
  );
}

/** One description, language and region for every realm that is not listed yet. */
export function PublishAllDialog({ realms, onClose }: { realms: PublishRealm[]; onClose: () => void }) {
  const t = useT();
  const { status, refresh } = useRegistryStatus();
  const [description, setDescription] = useState("");
  const [language, setLanguage] = useState("en");
  const [region, setRegion] = useState("");
  const [allowPortable, setAllowPortable] = useState(true);
  const [existingOnly, setExistingOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const todo = realms.filter((r) => !isListed(status?.realms.find((s) => s.local_id === r.localId)));

  async function publishAll() {
    setBusy(true); setError(null);
    try {
      for (const r of todo) await realmRegistry.publish(r.localId, r.defaultName, description, language, region.trim() === "" ? null : region.trim());
      // The joining options are saved realm by realm; one that cannot be saved now (for example the server is running) does not stop the others.
      const failed: string[] = [];
      for (const r of todo) {
        try { await control.setAccess(r.localId, existingOnly, allowPortable, null); }
        catch (e) { failed.push(`${r.label}: ${asControlError(e).message}`); }
      }
      await refresh();
      if (failed.length === 0) onClose();
      else setError(t("registry.accessFailed", { reason: failed.join(" · ") }));
    } catch (e) { setError(asUiError(e).technical); await refresh(); } finally { setBusy(false); }
  }

  return (
    <Modal title={t("registry.publishAll")} onClose={onClose}>
      <p className="text-sm text-muted">{t("registry.publishAllText")}</p>
      <ul className="mt-3 space-y-1 text-sm">
        {realms.map((r) => <li key={r.localId} className="flex justify-between gap-3"><span>{r.defaultName}</span><span className="text-muted">{todo.includes(r) ? t("registry.short.disabled") : t("registry.alreadyListed")}</span></li>)}
      </ul>
      {todo.length > 0 && <div className="mt-4">
        <ListingFields idPrefix="pub-all" name="" setName={() => undefined} description={description} setDescription={setDescription} language={language} setLanguage={setLanguage} region={region} setRegion={setRegion} hideName />
        <JoinChoices existingOnly={existingOnly} setExistingOnly={setExistingOnly} allowPortable={allowPortable} setAllowPortable={setAllowPortable} />
        <div className="mt-4">
          <Button variant="primary" disabled={busy || !status?.url} onClick={() => void publishAll()}>
            {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("registry.publishAllButton", { n: todo.length })}
          </Button>
          {!status?.url && <p className="mt-2 text-xs text-muted">{t("registry.noUrl")}</p>}
        </div>
      </div>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
    </Modal>
  );
}
