import { useState } from "react";
import { Archive, Bot, Gauge, Globe, Server, Settings, Terminal, Users, type LucideIcon } from "lucide-react";
import { type ServerSummary } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Overview } from "@/screens/Overview";
import { Placeholder } from "@/screens/Placeholder";
import { SettingsPage } from "@/screens/SettingsPage";
import { BackupsPage } from "@/screens/BackupsPage";
import { PlayersPage } from "@/screens/PlayersPage";
import { Button } from "@/components/ui/button";

type Page = "overview" | "bots" | "server" | "players" | "friends" | "backups" | "console" | "settings";

const NAV: { id: Page; label: string; icon: LucideIcon; question: string }[] = [
  { id: "overview", label: "Overview", icon: Gauge, question: "Is my server running?" },
  { id: "bots", label: "Bots", icon: Bot, question: "How are my bots configured?" },
  { id: "server", label: "Server", icon: Server, question: "How does my server behave?" },
  { id: "players", label: "Players", icon: Users, question: "Who is playing right now?" },
  { id: "friends", label: "Play with Friends", icon: Globe, question: "How can my friend join?" },
  { id: "backups", label: "Backups", icon: Archive, question: "Is my data safe?" },
  { id: "console", label: "Console", icon: Terminal, question: "What is the server doing?" },
  { id: "settings", label: "Settings", icon: Settings, question: "How does Manager behave?" },
];

export function Shell(props: {
  servers: ServerSummary[];
  activeId: string;
  onSelect: (id: string) => void;
  onAddAnother: () => void;
  onForget: (id: string) => Promise<void>;
}) {
  const [page, setPage] = useState<Page>("overview");
  const server = props.servers.find((s) => s.id === props.activeId)!;
  const current = NAV.find((n) => n.id === page)!;

  return (
    <div className="flex h-full">
      <nav className="flex w-60 shrink-0 flex-col border-r border-line bg-[#0b0c0e] p-3" aria-label="Main">
        <div className="px-3 pb-4 pt-2 text-sm font-semibold tracking-wide text-gold">CoA Server Manager</div>
        <ul className="flex flex-col gap-0.5">
          {NAV.map(({ id, label, icon: Icon }) => (
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
                {label}
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
              aria-label="Server"
            >
              {props.servers.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          )}
          <Button variant="ghost" size="sm" className="w-full justify-start" onClick={props.onAddAnother}>
            Manage another server
          </Button>
        </div>
      </nav>

      <main className="h-full flex-1 overflow-y-auto px-10 py-8">
        {page === "overview" ? (
          <Overview key={server.id} server={server} onForget={() => props.onForget(server.id)} />
        ) : page === "bots" || page === "server" ? (
          <SettingsPage key={`${server.id}-${page}`} serverId={server.id} scope={page} title={current.label} question={current.question} />
        ) : page === "players" ? (
          <PlayersPage key={server.id} serverId={server.id} />
        ) : page === "backups" ? (
          <BackupsPage key={server.id} serverId={server.id} />
        ) : (
          <Placeholder title={current.label} question={current.question} />
        )}
      </main>
    </div>
  );
}
