import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { useT, type Key } from "@/i18n";
import { useRegistryStatus } from "@/lib/registry";
import { isListed, stateTone, usePublishRealms, type PublishRealm } from "@/lib/publishRealms";
import type { RealmPublishStatus } from "@/lib/registry";
import { PublishAllDialog, PublishRealmDialog } from "@/screens/PublishDialog";

/** A small button on a realm card: the listing state of that realm; it opens the window to publish or configure it. */
export function ListingChip({ status, hasUrl, onOpen }: { status: RealmPublishStatus | undefined; hasUrl: boolean; onOpen: () => void }) {
  const t = useT();
  const listed = isListed(status);
  if (!listed && !hasUrl) return null;
  return (
    <button
      type="button"
      onClick={onOpen}
      className={cn("inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1 text-xs font-medium normal-case tracking-normal transition-colors", listed ? "border-line bg-card-2 hover:border-gold/40" : "border-gold/40 text-gold hover:bg-gold/10")}
      title={t("registry.title")}
    >
      {listed ? <span className={stateTone(status?.state)}>{t(`registry.short.${status!.state}` as Key)}</span> : t("registry.publishShort")}
    </button>
  );
}

/** The public listing of this server's realms: one line per realm; the form opens in a window. */
export function PublishCard({ serverId, serverName }: { serverId: string; serverName: string }) {
  const t = useT();
  const realms = usePublishRealms(serverId, serverName);
  const { status } = useRegistryStatus();
  const [open, setOpen] = useState<PublishRealm | "all" | null>(null);
  if (!realms) return null;
  const listed = (r: PublishRealm) => isListed(status?.realms.find((s) => s.local_id === r.localId));
  const someUnlisted = realms.some((r) => !listed(r));

  return (
    <Card className="mt-6 p-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="font-semibold">{t("registry.title")}</h2>
        {realms.length > 1 && someUnlisted && <Button size="sm" onClick={() => setOpen("all")}>{t("registry.publishAll")}</Button>}
      </div>
      <p className="mt-1 text-sm text-muted">{t("registry.text")}</p>
      <ul className="mt-4 divide-y divide-line border-y border-line">
        {realms.map((r) => {
          const mine = status?.realms.find((s) => s.local_id === r.localId);
          return (
            <li key={r.localId} className="flex flex-wrap items-center justify-between gap-3 py-3">
              <div>
                <p className="font-medium">{r.label}</p>
                <p className={cn("text-xs", stateTone(isListed(mine) ? mine?.state : undefined))} data-state={mine?.state}>{t(`registry.short.${isListed(mine) ? mine!.state : "disabled"}` as Key)}</p>
              </div>
              <Button size="sm" onClick={() => setOpen(r)}>{isListed(mine) ? t("registry.configure") : t("registry.publishShort")}</Button>
            </li>
          );
        })}
      </ul>
      {!status?.url && <p className="mt-3 text-xs text-muted">{t("registry.noUrl")}</p>}
      {open === "all" && <PublishAllDialog realms={realms} onClose={() => setOpen(null)} />}
      {open && open !== "all" && <PublishRealmDialog realm={open} onClose={() => setOpen(null)} />}
    </Card>
  );
}
