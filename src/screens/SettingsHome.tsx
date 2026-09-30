import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { api, asUiError, type UiError, type UpdatePreview, type UpdateTxn } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ClientCard } from "@/screens/ClientCard";

const ACTION_TEXT: Record<string, string> = {
  create: "New file",
  replace: "Updated",
  "merge-config": "New settings added",
  skip: "Unchanged",
  conflict: "Needs your decision",
};

function mb(bytes: number) {
  return bytes >= 1 << 20 ? `${(bytes / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function SettingsHome({ serverId }: { serverId: string }) {
  const [preview, setPreview] = useState<UpdatePreview | null>(null);
  const [pending, setPending] = useState<UpdateTxn | null>(null);
  const [busy, setBusy] = useState<"check" | "update" | "rollback" | null>(null);
  const [progress, setProgress] = useState<{ step: string; percent: number } | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [choices, setChoices] = useState<Record<string, "keep" | "replace">>({});
  const [advanced, setAdvanced] = useState(false);
  const [source, setSource] = useState("");

  useEffect(() => {
    void api.pendingUpdate(serverId).then(setPending).catch(() => {});
    const un = listen<{ step: string; percent: number }>("update-progress", (e) => setProgress(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, [serverId]);

  async function check() {
    setBusy("check");
    setError(null);
    setDone(null);
    setPreview(null);
    try {
      const p = await api.checkUpdate(serverId, source.trim() || undefined);
      setPreview(p);
      setChoices(Object.fromEntries(p.conflicts.map((c) => [c, "keep" as const])));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  async function update() {
    setBusy("update");
    setError(null);
    setProgress({ step: "Starting", percent: 0 });
    try {
      const out = await api.applyUpdate(serverId, choices, source.trim() || undefined);
      setPreview(null);
      if (out.txn.state === "committed") setDone(`Updated to version ${out.txn.to_version}.`);
      else setPending(out.txn);
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
      setProgress(null);
      void api.pendingUpdate(serverId).then(setPending).catch(() => {});
    }
  }

  async function rollback(t: UpdateTxn) {
    if (!window.confirm("Go back to the previous version? Your characters and accounts are not changed.")) return;
    setBusy("rollback");
    setError(null);
    try {
      await api.rollbackUpdate(serverId, t.id);
      setPending(null);
      setDone("Went back to the previous version. Your data was not touched.");
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  async function browse() {
    const p = await open({ directory: true, multiple: false, title: "Choose an update package folder" });
    if (typeof p === "string") setSource(p);
  }

  return (
    <div className="max-w-2xl">
      <h1 className="text-2xl font-semibold">Settings</h1>
      <p className="mt-1 text-muted">How does Manager behave?</p>

      {pending && (
        <Card className="mt-6 border-warn/40 p-5" role="alert">
          <p className="font-medium text-warn">
            {pending.state === "needs-decision" ? "The updated server did not start correctly" : "An earlier update did not finish"}
          </p>
          <p className="mt-1 text-sm text-muted">{pending.message ?? "You can safely go back to the previous version."}</p>
          <p className="mt-1 text-sm text-muted">Going back restores program files and settings. Characters and accounts are never rolled back automatically.</p>
          <Button className="mt-3" size="sm" disabled={!!busy} onClick={() => void rollback(pending)}>
            {busy === "rollback" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            Go back to version {pending.from_version ?? "before the update"}
          </Button>
        </Card>
      )}

      <ClientCard serverId={serverId} />

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">Server updates</h2>
        <p className="mt-1 text-sm text-muted">Updates are checked for authenticity, backed up first, and can be undone.</p>

        <div className="mt-4 flex items-center gap-3">
          <Button variant="primary" disabled={!!busy} onClick={() => void check()}>
            {busy === "check" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            Check for updates
          </Button>
          <button onClick={() => setAdvanced((v) => !v)} aria-expanded={advanced} className="flex cursor-pointer items-center gap-1 text-sm text-muted hover:text-ink">
            {advanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
            Advanced
          </button>
        </div>
        {advanced && (
          <div className="mt-3 flex gap-2">
            <input aria-label="Update package folder" value={source} onChange={(e) => setSource(e.target.value)} placeholder="Update package folder (leave empty to use the official source)" className="selectable flex-1 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold" />
            <Button size="sm" onClick={browse}>Browse…</Button>
          </div>
        )}

        {done && <p className="mt-4 text-sm text-ok" role="status">{done}</p>}
        {error && (
          <p className="mt-4 text-sm text-bad" role="alert">
            {error.human.code === "unknown" ? error.technical : error.human.message}
          </p>
        )}

        {preview && (
          <div className="mt-5 border-t border-line pt-4">
            <p className="font-medium">
              Version {preview.to_version} is available{preview.from_version ? ` (you have ${preview.from_version})` : ""}
            </p>
            <p className="mt-1 text-sm text-muted">
              Download {mb(preview.download_bytes)} · {preview.items.filter((i) => i.action !== "skip").length} file(s) change
              {preview.migrations > 0 ? ` · ${preview.migrations} database update(s)` : ""}
            </p>
            <ul className="mt-3 max-h-56 divide-y divide-line overflow-auto text-sm">
              {preview.items.filter((i) => i.action !== "skip").map((i) => (
                <li key={i.path} className="py-2">
                  <div className="flex justify-between gap-3">
                    <span className="selectable break-all">{i.path}</span>
                    <span className={i.action === "conflict" ? "shrink-0 text-warn" : "shrink-0 text-muted"}>{ACTION_TEXT[i.action]}</span>
                  </div>
                  {i.action === "conflict" && (
                    <fieldset className="mt-2 flex flex-wrap gap-4 text-xs text-muted">
                      <legend className="sr-only">This file was modified outside CoA Server Manager</legend>
                      <span className="text-warn">This file was modified outside CoA Server Manager.</span>
                      {(["keep", "replace"] as const).map((c) => (
                        <label key={c} className="flex cursor-pointer items-center gap-1">
                          <input type="radio" name={i.path} checked={choices[i.path] === c} onChange={() => setChoices((x) => ({ ...x, [i.path]: c }))} />
                          {c === "keep" ? "Keep my file" : "Replace with update"}
                        </label>
                      ))}
                    </fieldset>
                  )}
                </li>
              ))}
            </ul>
            <Button className="mt-4" variant="primary" disabled={!!busy} onClick={() => void update()}>
              {busy === "update" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              Update now
            </Button>
            <p className="mt-2 text-xs text-muted">The server is stopped during the update and started again to check that it works.</p>
          </div>
        )}

        {busy === "update" && progress && (
          <div className="mt-5">
            <div className="flex justify-between text-sm">
              <span role="status">{progress.step}</span>
              <span className="text-muted">{progress.percent}%</span>
            </div>
            <div className="mt-2 h-2 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={progress.percent} aria-valuemin={0} aria-valuemax={100}>
              <div className="h-full rounded-full bg-gold transition-[width] duration-300" style={{ width: `${progress.percent}%` }} />
            </div>
          </div>
        )}
      </Card>
    </div>
  );
}
