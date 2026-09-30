import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, asUiError, type ClientInfo, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

export function ClientCard({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [info, setInfo] = useState<ClientInfo | null | undefined>(undefined);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => api.clientInfo(serverId).then(setInfo).catch((e) => setError(asUiError(e)));
  useEffect(() => {
    void refresh();
  }, [serverId]); // eslint-disable-line react-hooks/exhaustive-deps

  async function run(fn: () => Promise<string | void>) {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const msg = await fn();
      if (msg) setNote(msg);
      await refresh();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  async function choose() {
    const picked = await open({ directory: true, multiple: false, title: t("client.dialogTitle") });
    if (typeof picked === "string") await run(async () => void (await api.setClient(serverId, picked)));
  }

  const hosts = info?.realmlists.map((r) => r.host) ?? [];
  const pointsLocal = hosts.length > 0 && hosts.every((h) => h === "127.0.0.1" || h === "localhost");

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">{t("client.title")}</h2>
      {info === undefined && <p className="mt-2 text-sm text-muted">{t("client.looking")}</p>}
      {info === null && (
        <>
          <p className="mt-1 text-sm text-muted">{t("client.intro")}</p>
          <Button className="mt-4" onClick={() => void choose()} disabled={busy}>{t("client.chooseFolder")}</Button>
        </>
      )}
      {info && (
        <>
          <p className="selectable mt-1 break-all text-sm text-muted">{info.path}</p>
          <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
            <dt className="text-muted">{t("client.connectsTo")}</dt>
            <dd>
              {hosts.length ? hosts.filter(Boolean).join(", ") : "—"}
              {pointsLocal ? <span className="ml-2 text-ok">{t("client.thisComputer")}</span> : <span className="ml-2 text-warn">{t("client.notThisServer")}</span>}
            </dd>
            <dt className="text-muted">{t("client.addon")}</dt>
            <dd>
              {info.addon.installed ? (info.addon.version ? t("client.installedVersion", { v: info.addon.version }) : t("client.installed")) : t("client.addonNotInstalled")}
              {info.addon.up_to_date === false && <span className="ml-2 text-warn">{t("client.updateAvailable")}</span>}
            </dd>
            <dt className="text-muted">{t("client.otherAddons")}</dt>
            <dd>{t("client.leftAsIs", { n: info.other_addons })}</dd>
          </dl>
          <div className="mt-4 flex flex-wrap gap-2">
            {!pointsLocal && (
              <Button size="sm" disabled={busy} onClick={() => void run(async () => { await api.setRealmlist(serverId, "127.0.0.1"); return t("client.pointedNote"); })}>
                {t("client.pointLocal")}
              </Button>
            )}
            {(!info.addon.installed || info.addon.up_to_date === false) && (
              <Button size="sm" disabled={busy} onClick={() => void run(async () => { await api.installAddon(serverId); return t("client.addonNote"); })}>
                {info.addon.installed ? t("client.updateAddon") : t("client.installAddon")}
              </Button>
            )}
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => void choose()}>{t("client.changeFolder")}</Button>
          </div>
        </>
      )}
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </Card>
  );
}
