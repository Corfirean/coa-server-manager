import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

export function AboutCard() {
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [busy, setBusy] = useState<"check" | "install" | null>(null);
  const [percent, setPercent] = useState<number | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getVersion().then(setVersion).catch(() => {});
  }, []);

  async function look() {
    setBusy("check");
    setError(null);
    setNote(null);
    try {
      const u = await check();
      setUpdate(u);
      if (!u) setNote("CoA Server Manager is up to date.");
    } catch {
      setError("Could not check for a new version. Check your internet connection and try again.");
    } finally {
      setBusy(null);
    }
  }

  async function install() {
    if (!update) return;
    setBusy("install");
    setError(null);
    let total = 0;
    let done = 0;
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") total = e.data.contentLength ?? 0;
        if (e.event === "Progress") {
          done += e.data.chunkLength;
          if (total) setPercent(Math.round((done / total) * 100));
        }
      });
      await relaunch();
    } catch {
      setError("The update could not be installed. Your server was not touched.");
      setBusy(null);
    }
  }

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">CoA Server Manager</h2>
      <p className="mt-1 text-sm text-muted">Version {version || "…"}. Updates to the program never change your server.</p>
      <div className="mt-4 flex items-center gap-3">
        <Button size="sm" disabled={!!busy} onClick={() => void look()}>
          {busy === "check" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          Check for a new version
        </Button>
        {update && (
          <Button size="sm" variant="primary" disabled={!!busy} onClick={() => void install()}>
            {busy === "install" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            Install {update.version} and restart
          </Button>
        )}
        {percent !== null && busy === "install" && <span className="text-sm text-muted" role="status">{percent}%</span>}
      </div>
      {update?.body && <p className="mt-3 whitespace-pre-line text-sm text-muted">{update.body}</p>}
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
    </Card>
  );
}
