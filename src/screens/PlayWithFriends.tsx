import { useState } from "react";
import { Globe, Network } from "lucide-react";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { BrowsePage } from "@/screens/BrowsePage";
import { FriendsPage } from "@/screens/FriendsPage";

export function PlayWithFriends({ serverId }: { serverId: string; path?: string }) {
  const t = useT();
  const [tab, setTab] = useState<"servers" | "lan">("servers");

  return (
    <div className="space-y-4">
      <div className="flex border-b border-line pb-2" role="tablist">
        <button
          role="tab"
          aria-selected={tab === "servers"}
          onClick={() => setTab("servers")}
          className={cn(
            "flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors cursor-pointer",
            tab === "servers" ? "bg-white/[0.08] font-medium text-ink" : "text-muted hover:bg-white/[0.04] hover:text-ink"
          )}
        >
          <Globe className="h-4 w-4" aria-hidden />
          {t("fr.tab.servers")}
        </button>
        <button
          role="tab"
          aria-selected={tab === "lan"}
          onClick={() => setTab("lan")}
          className={cn(
            "ml-2 flex items-center gap-2 rounded-md px-3 py-1.5 text-sm transition-colors cursor-pointer",
            tab === "lan" ? "bg-white/[0.08] font-medium text-ink" : "text-muted hover:bg-white/[0.04] hover:text-ink"
          )}
        >
          <Network className="h-4 w-4" aria-hidden />
          {t("fr.tab.lan")}
        </button>
      </div>

      {tab === "servers" && <BrowsePage />}
      {tab === "lan" && <FriendsPage serverId={serverId} />}
    </div>
  );
}
