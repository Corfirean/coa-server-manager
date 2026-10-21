import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, asUiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";

/** Where a Docker server reads the game data from. Not shown for a repack, which keeps its data in its own folder. */
export function GameDataCard({ serverId }: { serverId: string }) {
  const t = useT();
  const [folder, setFolder] = useState<string | null>(null);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    let alive = true;
    void api.gameDataFolder(serverId).then((f) => alive && setFolder(f)).catch(() => alive && setFolder(null));
    return () => { alive = false; };
  }, [serverId]);

  if (folder === null) return null;

  async function change() {
    const picked = await open({ directory: true, multiple: false, title: t("gamedata.dialog"), defaultPath: folder ?? undefined });
    if (typeof picked !== "string") return;
    try {
      setFolder(await api.setGameDataFolder(serverId, picked));
      setMessage({ ok: true, text: t("gamedata.saved") });
    } catch (e) {
      setMessage({ ok: false, text: asUiError(e).technical });
    }
  }

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">{t("gamedata.title")}</h2>
      <p className="mt-1 text-sm text-muted">{t("gamedata.text")}</p>
      <div className="mt-3 flex items-center gap-3">
        <code className="selectable min-w-0 flex-1 truncate rounded-md border border-line bg-card px-3 py-2 text-sm" title={folder}>{folder}</code>
        <Button onClick={() => void change()}>{t("gamedata.change")}</Button>
      </div>
      {message && <p className={message.ok ? "mt-2 text-sm text-ok" : "mt-2 text-sm text-bad"} role="status">{message.text}</p>}
    </Card>
  );
}
