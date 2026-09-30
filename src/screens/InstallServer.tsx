import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { api, asUiError, type InstallStep, type Preflight, type ServerSummary, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

type Phase = "choose" | "installing" | "done";

export function InstallServer(props: { canCancel: boolean; onCancel: () => void; onDone: (s: ServerSummary) => void | Promise<void> }) {
  const [dest, setDest] = useState("C:\\Games\\CoA Server");
  const [pre, setPre] = useState<Preflight | null>(null);
  const [phase, setPhase] = useState<Phase>("choose");
  const [step, setStep] = useState<InstallStep>({ step: "Preparing your server", percent: 0, detail: null });
  const [error, setError] = useState<UiError | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [pkg, setPkg] = useState("");
  const [installed, setInstalled] = useState<ServerSummary | null>(null);
  const [showDetails, setShowDetails] = useState(false);

  useEffect(() => {
    const t = setTimeout(() => void api.installPreflight(dest).then(setPre).catch(() => setPre(null)), 250);
    return () => clearTimeout(t);
  }, [dest]);

  useEffect(() => {
    const un = listen<InstallStep>("install-progress", (e) => setStep(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, []);

  async function browse() {
    const picked = await open({ directory: true, multiple: false, title: "Choose where to install your server" });
    if (typeof picked === "string") setDest(picked.replace(/[\\/]+$/, "") + (picked.toLowerCase().includes("coa") ? "" : "\\CoA Server"));
  }

  async function browsePackage() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a server package folder" });
    if (typeof picked === "string") setPkg(picked);
  }

  async function install() {
    setError(null);
    setPhase("installing");
    setStep({ step: "Preparing your server", percent: 0, detail: null });
    try {
      const s = await api.installNew(dest, pkg.trim() || undefined);
      setInstalled(s);
      setPhase("done");
    } catch (e) {
      setError(asUiError(e));
      setPhase("choose");
    }
  }

  if (phase === "installing") {
    return (
      <main className="mx-auto flex h-full max-w-xl flex-col justify-center px-8">
        <h1 className="text-2xl font-semibold">Preparing your server</h1>
        <p className="mt-1 text-muted">This can take a few minutes. You can leave this window open.</p>
        <Card className="mt-8 p-6">
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-2 font-medium">
              <Loader2 className="h-4 w-4 animate-spin text-gold" aria-hidden /> {step.step}
            </span>
            <span className="text-sm text-muted" role="status">{step.percent}%</span>
          </div>
          <div className="mt-3 h-2 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuenow={step.percent} aria-valuemin={0} aria-valuemax={100}>
            <div className="h-full rounded-full bg-gold transition-[width] duration-300" style={{ width: `${step.percent}%` }} />
          </div>
          {step.detail && <p className="mt-2 text-xs text-muted">{step.detail}</p>}
        </Card>
        {step.step === "Downloading server" && (
          <Button className="mt-4 self-start" variant="ghost" size="sm" onClick={() => void api.cancelInstall()}>
            Cancel
          </Button>
        )}
      </main>
    );
  }

  if (phase === "done" && installed) {
    return (
      <main className="mx-auto flex h-full max-w-xl flex-col items-center justify-center px-8 text-center">
        <div className="flex h-14 w-14 items-center justify-center rounded-full bg-ok/15">
          <Check className="h-7 w-7 text-ok" aria-hidden />
        </div>
        <h1 className="mt-5 text-2xl font-semibold">Your server is ready</h1>
        <p className="mt-1 text-muted">Everything is installed. Start it from the next screen.</p>
        <Button className="mt-8" variant="primary" size="xl" onClick={() => void props.onDone(installed)}>
          OPEN MY SERVER
        </Button>
      </main>
    );
  }

  return (
    <main className="mx-auto flex h-full max-w-2xl flex-col justify-center gap-5 px-8 py-10">
      <div>
        <h1 className="text-2xl font-semibold">Install a new server</h1>
        <p className="mt-1 text-muted">Choose a folder, or keep the suggested one. Everything else is set up for you.</p>
      </div>

      <div>
        <label htmlFor="dest" className="text-sm text-muted">Install to</label>
        <div className="mt-1 flex gap-2">
          <input
            id="dest"
            value={dest}
            onChange={(e) => setDest(e.target.value)}
            className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2.5 text-[15px] outline-none focus:border-gold"
          />
          <Button onClick={browse}>Browse…</Button>
        </div>
      </div>

      {pre && pre.problems.length > 0 && (
        <Card className="border-warn/40 p-4" role="alert">
          {pre.problems.map((p) => (
            <p key={p.code} className="text-sm text-warn">{p.message}</p>
          ))}
        </Card>
      )}
      {pre?.ok && <p className="text-sm text-ok">This folder is ready. {Math.round(pre.free_bytes / (1 << 30))} GB free.</p>}

      {error && (
        <Card className="border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{error.human.code === "unknown" ? "The installation could not finish" : error.human.title}</p>
          <p className="mt-1 text-sm text-muted">{error.human.code === "unknown" ? error.technical : error.human.message}</p>
          <p className="mt-1 text-sm text-muted">Nothing was installed. You can try again; downloaded files are kept.</p>
          <button className="mt-2 cursor-pointer text-xs text-muted underline" onClick={() => setShowDetails((v) => !v)}>
            {showDetails ? "Hide" : "Show"} technical details
          </button>
          {showDetails && <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-2 text-xs">{error.technical}</pre>}
        </Card>
      )}

      <div>
        <button onClick={() => setShowAdvanced((v) => !v)} aria-expanded={showAdvanced} className="flex cursor-pointer items-center gap-1 text-sm text-muted hover:text-ink">
          {showAdvanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
          Advanced
        </button>
        {showAdvanced && (
          <div className="mt-2">
            <label htmlFor="pkg" className="text-sm text-muted">Install from a local package folder instead of downloading</label>
            <div className="mt-1 flex gap-2">
              <input id="pkg" value={pkg} onChange={(e) => setPkg(e.target.value)} placeholder="(leave empty to download)" className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2 text-sm outline-none focus:border-gold" />
              <Button size="sm" onClick={browsePackage}>Browse…</Button>
            </div>
          </div>
        )}
      </div>

      <div className="flex items-center gap-3">
        <Button variant="primary" size="md" disabled={!pre?.ok} onClick={() => void install()}>
          Install
        </Button>
        {props.canCancel && (
          <Button variant="ghost" onClick={props.onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </main>
  );
}
