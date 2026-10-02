import { useCallback, useEffect, useState } from "react";
import { Archive, Bot, Bug, Gauge, Globe, Puzzle, Server, Settings, Terminal, Users, type LucideIcon } from "lucide-react";
import { api, type ServerSummary } from "@/lib/api";
import { useT, type Key } from "@/i18n";
import logo from "@/assets/logo.png";
import { cn } from "@/lib/utils";
import { Overview } from "@/screens/Overview";
import { Placeholder } from "@/screens/Placeholder";
import { SettingsPage } from "@/screens/SettingsPage";
import { BackupsPage } from "@/screens/BackupsPage";
import { PlayersPage } from "@/screens/PlayersPage";
import { SettingsHome } from "@/screens/SettingsHome";
import { FriendsPage } from "@/screens/FriendsPage";
import { ReportPage } from "@/screens/ReportPage";
import { ModulesPage } from "@/screens/ModulesPage";
import { ConsolePage } from "@/screens/ConsolePage";
import { Button } from "@/components/ui/button";
import { startServerUpdatePolling, useServerUpdate } from "@/lib/serverUpdate";
import { startClientPolling } from "@/lib/clientUpdate";

type Page = "overview" | "bots" | "server" | "modules" | "players" | "friends" | "backups" | "console" | "report" | "settings";

const NAV: { id: Page; label: Key; icon: LucideIcon; question: Key }[] = [
  { id: "overview", label: "nav.overview", icon: Gauge, question: "q.overview" },
  { id: "bots", label: "nav.bots", icon: Bot, question: "q.bots" },
  { id: "server", label: "nav.server", icon: Server, question: "q.server" },
  { id: "modules", label: "nav.modules", icon: Puzzle, question: "q.modules" },
  { id: "players", label: "nav.players", icon: Users, question: "q.players" },
  { id: "friends", label: "nav.friends", icon: Globe, question: "q.friends" },
  { id: "backups", label: "nav.backups", icon: Archive, question: "q.backups" },
  { id: "console", label: "nav.console", icon: Terminal, question: "q.console" },
  { id: "report", label: "nav.report", icon: Bug, question: "q.report" },
  { id: "settings", label: "nav.settings", icon: Settings, question: "q.settings" },
];

export function Shell(props: {
  servers: ServerSummary[];
  activeId: string;
  onSelect: (id: string) => void;
  onAddAnother: () => void;
  onForget: (id: string) => Promise<void>;
}) {
  const t = useT();
  const [page, setPage] = useState<Page>("overview");
  const server = props.servers.find((s) => s.id === props.activeId)!;
  const current = NAV.find((n) => n.id === page)!;
  const update = useServerUpdate(props.activeId);

  // A module that is switched off takes its page away (the companions' "Bots" page, for one).
  const [hiddenPages, setHiddenPages] = useState<string[]>([]);
  const reloadModules = useCallback(() => {
    void api
      .modulesList(props.activeId)
      .then((list) => setHiddenPages(list.filter((m) => m.page && m.installed && m.switchable && !m.enabled).map((m) => m.page as string)))
      .catch(() => setHiddenPages([]));
  }, [props.activeId]);
  useEffect(() => {
    reloadModules();
  }, [reloadModules]);
  useEffect(() => {
    if (hiddenPages.includes(page)) setPage("overview");
  }, [hiddenPages, page]);

  // Look for a server and a game client update when the app starts and then every five minutes.
  useEffect(() => {
    const stopServer = startServerUpdatePolling(props.activeId);
    const stopClient = startClientPolling(props.activeId);
    return () => {
      stopServer();
      stopClient();
    };
  }, [props.activeId]);

  return (
    <div className="flex h-full">
      <nav className="flex w-60 shrink-0 flex-col border-r border-line bg-[#0b0c0e] p-3" aria-label={t("nav.main")}>
        <div className="flex items-center gap-2 px-3 pb-4 pt-2 text-sm font-semibold tracking-wide text-gold">
          <img src={logo} alt="" aria-hidden className="h-7 w-7" />
          {t("app.name")}
        </div>
        <ul className="flex flex-col gap-0.5">
          {NAV.filter((n) => !hiddenPages.includes(n.id)).map(({ id, label, icon: Icon }) => (
            <li key={id}>
              <button
                onClick={() => setPage(id)}
                aria-current={page === id ? "page" : undefined}
                className={cn(
                  "flex h-10 w-full cursor-pointer items-center gap-3 rounded-md px-3 text-left text-[15px] transition-colors",
                  page === id ? "bg-white/[0.07] text-ink" : "text-muted hover:bg-white/5 hover:text-ink",
                )}
              >
                <Icon className={cn("h-[18px] w-[18px]", page === id && "text-gold")} aria-hidden />
                {t(label)}
                {id === "settings" && update.available && (
                  <span className="ml-auto h-2 w-2 rounded-full bg-gold" role="img" aria-label={t("nav.updateDot")} title={t("nav.updateDot")} />
                )}
              </button>
            </li>
          ))}
        </ul>

        <div className="mt-auto border-t border-line pt-3">
          {props.servers.length > 1 && (
            <select
              className="mb-2 w-full rounded-md border border-line bg-card px-2 py-2 text-sm"
              value={props.activeId}
              onChange={(e) => props.onSelect(e.target.value)}
              aria-label={t("nav.serverPicker")}
            >
              {props.servers.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          )}
          <Button variant="ghost" size="sm" className="w-full justify-start" onClick={props.onAddAnother}>
            {t("nav.addAnother")}
          </Button>
        </div>
      </nav>

      <main className="h-full flex-1 overflow-y-auto px-10 py-8">
        {page === "overview" ? (
          <Overview key={server.id} server={server} companions={!hiddenPages.includes("bots")} onForget={() => props.onForget(server.id)} onOpenUpdates={() => setPage("settings")} />
        ) : page === "bots" || page === "server" ? (
          <SettingsPage key={`${server.id}-${page}`} serverId={server.id} scope={page} title={t(current.label)} question={t(current.question)} />
        ) : page === "console" ? (
          <ConsolePage key={server.id} serverId={server.id} />
        ) : page === "modules" ? (
          <ModulesPage key={server.id} serverId={server.id} onChanged={reloadModules} />
        ) : page === "report" ? (
          <ReportPage key={server.id} serverId={server.id} />
        ) : page === "friends" ? (
          <FriendsPage key={server.id} serverId={server.id} />
        ) : page === "settings" ? (
          <SettingsHome key={server.id} serverId={server.id} />
        ) : page === "players" ? (
          <PlayersPage key={server.id} serverId={server.id} />
        ) : page === "backups" ? (
          <BackupsPage key={server.id} serverId={server.id} />
        ) : (
          <Placeholder title={t(current.label)} question={t(current.question)} />
        )}
      </main>
    </div>
  );
}
