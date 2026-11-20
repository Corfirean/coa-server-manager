import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, ExternalLink } from "lucide-react";
import { api, asUiError, REMOTE_CLIENT_ID, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

/** Where the community hands out its launcher, which downloads the game client. The Manager does not download it. */
export const CLIENT_SITE = "https://coa-development.org/";

type Phase = "choose" | "done" | "failed";

/**
 * Linking the game client: pick a folder that already has the game, or go to the community site for the official
 * launcher (it downloads the client after signing in) and come back to pick the folder it installed to.
 */
export function ClientDialog(props: {
  serverId: string;
  onClose: () => void;
  /** The linked client changed; the caller refreshes what it shows. */
  onChanged: () => void;
}) {
  const t = useT();
  const human = useHuman();
  const { serverId } = props;
  const [phase, setPhase] = useState<Phase>("choose");
  const [error, setError] = useState<UiError | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function close() {
    props.onChanged();
    props.onClose();
  }

  async function useExisting() {
    const picked = await open({ directory: true, multiple: false, title: t("client.dialogTitle") });
    if (typeof picked !== "string") return;
    setError(null);
    try {
      await api.setClient(serverId, picked);
      setPhase("done");
    } catch (e) {
      setError(asUiError(e));
      setPhase("failed");
    }
  }

  async function openSite() {
    try {
      await api.openLink(CLIENT_SITE);
    } catch (e) {
      setError(asUiError(e));
      setPhase("failed");
    }
  }

  const title = t("client.setup.title");

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" role="presentation">
      <Card className="max-h-full w-full max-w-lg overflow-y-auto p-6 shadow-2xl" role="dialog" aria-modal="true" aria-label={title}>
        <h2 className="text-lg font-semibold">{title}</h2>

        {phase === "choose" && (
          <>
            <p className="mt-1 text-sm text-muted">{t("client.setup.intro")}</p>
            <div className="mt-4 grid gap-3">
              <button type="button" onClick={() => void openSite()} className="cursor-pointer rounded-md border border-line bg-card-2 p-4 text-left transition-colors hover:border-gold/50">
                <span className="flex items-center gap-2 font-medium">
                  {t("client.setup.getLauncher")}
                  <ExternalLink className="h-4 w-4 text-muted" aria-hidden />
                </span>
                <span className="mt-1 block text-sm text-muted">{t("client.setup.getLauncherHint")}</span>
              </button>
              <button type="button" onClick={() => void useExisting()} className="cursor-pointer rounded-md border border-line bg-card-2 p-4 text-left transition-colors hover:border-gold/50">
                <span className="block font-medium">{t("client.setup.have")}</span>
                <span className="mt-1 block text-sm text-muted">{t("client.setup.haveHint")}</span>
              </button>
            </div>
            <p className="mt-3 text-xs text-muted">{t("client.setup.hdHint")}</p>
            <div className="mt-5 flex justify-end">
              <Button variant="ghost" onClick={close}>{t("btn.close")}</Button>
            </div>
          </>
        )}

        {phase === "done" && (
          <>
            <div className="mt-4 flex items-center gap-3">
              <span className="flex h-9 w-9 items-center justify-center rounded-full bg-ok/15 text-ok"><Check className="h-5 w-5" aria-hidden /></span>
              <div>
                <p className="font-medium">{t("client.dl.done")}</p>
                <p className="text-sm text-muted">{t(serverId === REMOTE_CLIENT_ID ? "remote.clientReady" : "client.dl.doneText")}</p>
              </div>
            </div>
            <div className="mt-5 flex justify-end">
              <Button variant="primary" onClick={close}>{t("btn.close")}</Button>
            </div>
          </>
        )}

        {phase === "failed" && error && (
          <>
            <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" onClick={close}>{t("btn.close")}</Button>
              <Button onClick={() => { setError(null); setPhase("choose"); }}>{t("client.dl.back")}</Button>
            </div>
          </>
        )}
      </Card>
    </div>
  );
}
