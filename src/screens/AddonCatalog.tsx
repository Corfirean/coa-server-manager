import { useEffect, useState } from "react";
import { ExternalLink, Loader2 } from "lucide-react";
import { api, asUiError, type AddonView } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";

function megabytes(bytes: number): string {
  return (bytes / 1024 / 1024).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0);
}

/** Addons from the Manager's catalog: each pinned to a commit of its author's repository and checked file by file. */
export function AddonCatalog({ serverId }: { serverId: string }) {
  const { t, locale } = useI18n();
  const [addons, setAddons] = useState<AddonView[] | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void api.addonsList(serverId).then(list => { if (alive) setAddons(list); }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);

  async function run(id: string, action: "install" | "remove") {
    setWorking(id); setError(null);
    try { setAddons(await (action === "install" ? api.addonsInstall : api.addonsRemove)(serverId, id)); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setWorking(null); }
  }

  return <div className="mt-5 border-t border-line pt-4">
    <h3 className="text-sm font-semibold">{t("addons.title")}</h3>
    <p className="mt-1 text-xs text-muted">{t("addons.hint")}</p>
    <ul className="mt-3 space-y-3">
      {addons?.map(a => <li key={a.id} className="rounded-md border border-line p-3">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{a.name}</span>
          <span className="text-xs text-muted">{t("addons.by", { author: a.author })} · {megabytes(a.size)} MB</span>
          {a.installed && !a.updateAvailable && <span className="text-xs text-ok">{t("addons.installed")}</span>}
          {a.updateAvailable && <span className="text-xs text-warn">{t("addons.update")}</span>}
          <button type="button" className="ml-auto text-muted hover:text-ink" title={a.repo} aria-label={`GitHub: ${a.name}`} onClick={() => void api.openLink(a.repo)}><ExternalLink className="h-4 w-4" aria-hidden /></button>
        </div>
        <p className="mt-1 text-sm text-muted">{a.description[locale] ?? a.description.en}</p>
        {a.needs === "playerbots" && <p className="mt-1 text-xs text-muted">{t("addons.needsSquid")}</p>}
        {a.foreignFolders.length > 0 && !a.installed && <p className="mt-1 text-xs text-warn">{t("addons.foreign", { folders: a.foreignFolders.join(", ") })}</p>}
        <div className="mt-2 flex gap-2">
          {(!a.installed || a.updateAvailable) && <Button size="sm" disabled={working !== null} onClick={() => void run(a.id, "install")}>
            {working === a.id && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {a.updateAvailable ? t("addons.doUpdate") : t("addons.doInstall")}
          </Button>}
          {a.installed && <Button size="sm" variant="ghost" disabled={working !== null} onClick={() => void run(a.id, "remove")}>{t("addons.doRemove")}</Button>}
        </div>
      </li>)}
    </ul>
    {error && <p className="mt-2 text-sm text-bad" role="alert">{error}</p>}
  </div>;
}
