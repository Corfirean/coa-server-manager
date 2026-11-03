import { useState } from "react";
import { Download, Power, FolderOpen, Wrench, Languages } from "lucide-react";
import { useI18n } from "@/i18n";
import { WindowSettings } from "./WindowSettings";
import { PublicServerSettings } from "./PublicServerSettings";
import { LanguagePicker } from "@/components/LanguagePicker";
import { Card } from "@/components/ui/card";
import { useServerUpdate } from "@/lib/serverUpdate";
import { ServerUpdateCard } from "./ServerUpdateCard";
import { AboutCard } from "./AboutCard";
import { ReleaseHistory, type HistorySelection } from "./ReleaseHistory";
import { ClientCard } from "./ClientCard";
import { GameDataCard } from "./GameDataCard";
import { RealmStartupSettings } from "./RealmStartupSettings";
import { AutoRestartSettings } from "./AutoRestartSettings";
import { DiagnosticsCard } from "./DiagnosticsCard";
import { RepairCard } from "./RepairCard";
import { RemoveServerCard } from "./RemoveServerCard";

const sections = [["updates", Download], ["startup", Power], ["client", FolderOpen], ["service", Wrench], ["interface", Languages]] as const;
export function SettingsHome({ serverId, path, onForget }: { serverId: string; serverName?: string; path: string; onForget: () => Promise<void> }) {
  const { t } = useI18n();
  const [section, setSection] = useState<(typeof sections)[number][0]>("updates");
  const [attention, setAttention] = useState(false);
  const update = useServerUpdate(serverId);
  const [history, setHistory] = useState<HistorySelection>(() => ({ component: "server", from: update.preview?.from_version ?? undefined, to: update.available ? update.preview?.to_version : undefined }));
  function showHistory(selection: HistorySelection) {
    setHistory(selection);
    requestAnimationFrame(() => {
      const element = document.getElementById("release-history");
      element?.scrollIntoView({ behavior: "smooth", block: "start" });
      element?.focus({ preventScroll: true });
    });
  }
  return <div className="mx-auto w-full max-w-5xl pb-8">
    <h1 className="text-2xl font-semibold">{t("settings.title")}</h1>
    <p className="mt-1 text-sm text-muted">{t("settings.organized")}</p>
    <nav aria-label={t("settings.title")} className="mt-6 flex flex-wrap gap-1 border-b border-line pb-3">
      {sections.map(([id, Icon]) => <button key={id} aria-current={section === id ? "page" : undefined} onClick={() => setSection(id)} className={`flex cursor-pointer items-center gap-2 rounded-md px-3 py-2 text-sm ${section === id ? "bg-gold/10 text-gold" : "text-muted hover:bg-white/5 hover:text-ink"}`}>
        <Icon className="h-4 w-4" aria-hidden />{t(`settings.section.${id}`)}
      </button>)}
    </nav>
    {attention && section !== "updates" && <button role="alert" onClick={() => setSection("updates")} className="mt-4 w-full cursor-pointer rounded-lg border border-warn/40 p-4 text-left text-sm text-warn">{t("settings.attention")}</button>}
    <div hidden={section !== "updates"}>
      <div className="grid items-start gap-x-5 lg:grid-cols-2">
        <ServerUpdateCard onAttention={setAttention} serverId={serverId} onHistory={(from, to) => showHistory({ component: "server", from: from ?? undefined, to })} />
        <AboutCard onHistory={() => showHistory({ component: "manager" })} />
      </div>
      <button onClick={() => setSection("client")} className="mt-5 flex w-full cursor-pointer items-center justify-between gap-3 rounded-lg border border-line px-5 py-4 text-left text-sm hover:bg-white/5"><span>{t("settings.clientLink")}</span><span aria-hidden>→</span></button>
      <ReleaseHistory selection={history} onSelect={setHistory} />
    </div>
    <div hidden={section !== "startup"} className="max-w-3xl"><RealmStartupSettings serverId={serverId} /><AutoRestartSettings serverId={serverId} /></div>
    <div hidden={section !== "client"} className="max-w-3xl"><ClientCard serverId={serverId} /><GameDataCard serverId={serverId} /></div>
    <div hidden={section !== "service"} className="max-w-3xl"><DiagnosticsCard serverId={serverId} /><RepairCard serverId={serverId} /><RemoveServerCard serverId={serverId} path={path} onForget={onForget} /></div>
    <div hidden={section !== "interface"} className="max-w-3xl"><Card className="mt-6 p-6"><h2 className="font-semibold">{t("settings.language")}</h2><p className="mt-1 text-sm text-muted">{t("settings.languageHint")}</p><LanguagePicker className="mt-4" /></Card><WindowSettings /><PublicServerSettings /></div>
  </div>;
}
