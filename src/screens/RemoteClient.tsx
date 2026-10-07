import { useEffect, useState } from "react";
import { Gauge, Globe, Settings, Swords } from "lucide-react";
import { api, asUiError, REMOTE_CLIENT_ID } from "@/lib/api";
import { checkClient, startClientPolling, useClientStatus } from "@/lib/clientUpdate";
import { ClientCard } from "@/screens/ClientCard";
import { ClientDialog, type ClientDialogMode } from "@/screens/ClientDialog";
import { AboutCard } from "@/screens/AboutCard";
import { PortablePage } from "@/screens/PortablePage";
import { BrowsePage } from "@/screens/BrowsePage";
import { LanguagePicker } from "@/components/LanguagePicker";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";
import { cn } from "@/lib/utils";
import logo from "@/assets/logo.png";

export function RemoteClient({ onHost }: { onHost: () => void }) {
  const t = useT();
  const human = useHuman();
  const [page, setPage] = useState<"servers" | "overview" | "characters" | "settings">("servers");
  const [host, setHost] = useState("");
  const [savedHost, setSavedHost] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [dialog, setDialog] = useState<ClientDialogMode | null>(null);
  const { status } = useClientStatus(REMOTE_CLIENT_ID);
  function fail(e: unknown) {
    const err = asUiError(e);
    setError(err.human.code === "unknown" ? err.technical : human(err.human).message);
  }
  useEffect(() => {
    void api.remoteConnection().then(p => { setHost(p.host); setSavedHost(p.host); }).catch(fail).finally(() => setLoading(false));
    return startClientPolling(REMOTE_CLIENT_ID);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { void checkClient(REMOTE_CLIENT_ID); }, [revision]);

  async function save() {
    setBusy(true); setError(null); setNote(null);
    try {
      const profile = await api.remoteConnect(host);
      setHost(profile.host); setSavedHost(profile.host); setRevision(n => n + 1);
      setNote(t("remote.saved"));
    } catch (e) { fail(e); } finally { setBusy(false); }
  }
  async function launch() {
    setBusy(true); setError(null); setNote(null);
    try { await api.play(REMOTE_CLIENT_ID); } catch (e) { fail(e); } finally { setBusy(false); }
  }
  function play() {
    if (!status?.linked) setDialog("setup");
    else if (status.update_available) setDialog("update");
    else void launch();
  }
  return <div className="flex h-full">
    <nav className="flex w-60 shrink-0 flex-col border-r border-line bg-[#0b0c0e] p-3" aria-label={t("nav.main")}>
      <div className="flex items-center gap-2 px-3 pb-4 pt-2 text-sm font-semibold tracking-wide text-gold"><img src={logo} alt="" className="h-7 w-7" />{t("app.name")}</div>
      <ul className="flex flex-col gap-0.5">
        {(["servers", "overview", "characters", "settings"] as const).map(id => { const Icon = id === "servers" ? Globe : id === "overview" ? Gauge : id === "characters" ? Swords : Settings; return <li key={id}>
          <button onClick={() => setPage(id)} aria-current={page === id ? "page" : undefined} className={cn("flex h-10 w-full cursor-pointer items-center gap-3 rounded-md px-3 text-left text-[15px]", page === id ? "bg-white/[0.07] text-ink" : "text-muted hover:bg-white/5 hover:text-ink")}><Icon className="h-[18px] w-[18px]" aria-hidden />{t(id === "servers" ? "nav.servers" : id === "overview" ? "nav.overview" : id === "characters" ? "nav.characters" : "nav.settings")}</button>
        </li>; })}
      </ul>
      <div className="mt-auto border-t border-line pt-3"><Button variant="ghost" size="sm" className="w-full justify-start" onClick={onHost}>{t("remote.hostInstead")}</Button></div>
    </nav>
    <main className="h-full flex-1 overflow-y-auto px-10 py-8">
      {page === "servers" ? <BrowsePage /> : page === "characters" ? <PortablePage /> : page === "settings" ? <><div className="flex items-center justify-between"><h1 className="text-2xl font-semibold">{t("nav.settings")}</h1><LanguagePicker /></div><AboutCard /></> : <>
        <div className="flex items-center justify-between"><h1 className="text-2xl font-semibold">{t("welcome.connect.title")}</h1><LanguagePicker /></div>
        <p className="mt-2 text-muted">{t("remote.intro")}</p>
        <Card className="mt-6 p-6">
          <form onSubmit={e => { e.preventDefault(); void save(); }}>
            <label htmlFor="remote-host" className="font-semibold">{t("remote.address")}</label>
            <p id="remote-host-hint" className="mt-1 text-sm text-muted">{t("remote.addressHint")}</p>
            <div className="mt-4 flex gap-3"><input id="remote-host" aria-describedby="remote-host-hint" value={host} onChange={e => { setHost(e.target.value); setNote(null); }} disabled={loading || busy} placeholder="192.168.1.10" className="min-w-0 flex-1 rounded-md border border-line bg-card px-3 py-2" /><Button type="submit" disabled={loading || busy || !host.trim()}>{t("remote.save")}</Button></div>
          </form>
          <p className="mt-3 text-sm text-muted">{t("remote.accountHint")}</p>
          <Button variant="primary" className="mt-4" disabled={loading || busy || !savedHost || host.trim() !== savedHost || status === null} onClick={play}>{t("btn.play")}</Button>
          {status?.linked && status.update_available && <Button variant="secondary" className="mt-4 ml-3" disabled={loading || busy || !savedHost || host.trim() !== savedHost} onClick={() => void launch()}>{t("btn.playWithoutUpdate")}</Button>}
          {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
          {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
        </Card>
        <ClientCard key={revision} serverId={REMOTE_CLIENT_ID} remote />
      </>}
    </main>
    {dialog && <ClientDialog serverId={REMOTE_CLIENT_ID} mode={dialog} onClose={() => setDialog(null)} onChanged={() => setRevision(n => n + 1)} onPlayAnyway={() => void launch()} />}
  </div>;
}
