import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { api, asUiError, type InstallEnvironment, type InstallRequirements, type InstallStep, type Preflight, type ServerSummary, type UiError } from "@/lib/api";
import { formatBytes } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT, type Key } from "@/i18n";

// The backend reports progress with these English step names; map the known ones to translations.
const STEP_KEYS: Record<string, Key> = {
  "Preparing your server": "install.progressTitle",
  "Checking your computer": "install.step.checking",
  "Downloading server": "install.step.downloading",
  Unpacking: "install.step.unpacking",
  "Preparing database": "install.step.database",
  Finishing: "install.step.finishing",
};

type Phase = "choose" | "installing" | "done";

export function InstallServer(props: { canCancel: boolean; onCancel: () => void; onDone: (s: ServerSummary) => void | Promise<void> }) {
  const t = useT();
  const human = useHuman();
  const [dest, setDest] = useState("C:\\Games\\CoA Server");
  const [pre, setPre] = useState<Preflight | null>(null);
  const [req, setReq] = useState<InstallRequirements | null>(null);
  const [phase, setPhase] = useState<Phase>("choose");
  const [step, setStep] = useState<InstallStep>({ step: "Preparing your server", percent: 0, detail: null });
  const [error, setError] = useState<UiError | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [pkg, setPkg] = useState("");
  const [installed, setInstalled] = useState<ServerSummary | null>(null);
  const [showDetails, setShowDetails] = useState(false);
  // What this computer installs. A repack (Windows) needs nothing more than a folder; Docker (Linux) also needs the game data
  // folder and a package, and Docker itself.
  const [env, setEnv] = useState<InstallEnvironment | null>(null);
  const [gameData, setGameData] = useState("");
  const docker = env?.flavor === "docker";

  function loadEnvironment(first: boolean) {
    void api.installEnvironment().then((e) => {
      setEnv(e);
      if (first && e.flavor === "docker") setDest(e.default_dir);
    }).catch(() => setEnv(null));
  }
  useEffect(() => loadEnvironment(true), []);

  // how big the server is, from the package's signed list (re-read when another package source is entered)
  useEffect(() => {
    const timer = setTimeout(() => void api.installRequirements(pkg.trim() || undefined).then(setReq).catch(() => setReq(null)), 400);
    return () => clearTimeout(timer);
  }, [pkg]);

  // room for the unpacked server plus the download that is removed afterwards
  const needed = req ? req.unpacked_bytes + req.download_bytes : undefined;
  useEffect(() => {
    const timer = setTimeout(() => void api.installPreflight(dest, needed, docker ? gameData.trim() : undefined).then(setPre).catch(() => setPre(null)), 250);
    return () => clearTimeout(timer);
  }, [dest, needed, docker, gameData]);

  useEffect(() => {
    const un = listen<InstallStep>("install-progress", (e) => setStep(e.payload));
    return () => {
      void un.then((f) => f());
    };
  }, []);

  async function browse() {
    const picked = await open({ directory: true, multiple: false, title: t("install.dialogTitle") });
    if (typeof picked === "string") setDest(picked.replace(/[\\/]+$/, "") + (picked.toLowerCase().includes("coa") ? "" : docker ? "/CoaServer" : "\\CoA Server"));
  }

  async function browseGameData() {
    const picked = await open({ directory: true, multiple: false, title: t("install.data.dialogTitle") });
    if (typeof picked === "string") setGameData(picked);
  }

  async function browsePackage() {
    const picked = await open({ directory: true, multiple: false, title: t("install.pkgDialogTitle") });
    if (typeof picked === "string") setPkg(picked);
  }

  async function install() {
    setError(null);
    setPhase("installing");
    setStep({ step: "Preparing your server", percent: 0, detail: null });
    try {
      const s = await api.installNew(dest, pkg.trim() || undefined, docker ? gameData.trim() : undefined);
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
        <h1 className="text-2xl font-semibold">{t("install.progressTitle")}</h1>
        <p className="mt-1 text-muted">{t("install.progressText")}</p>
        <Card className="mt-8 p-6">
          <div className="flex items-center justify-between">
            <span className="flex items-center gap-2 font-medium">
              <Loader2 className="h-4 w-4 animate-spin text-gold" aria-hidden /> {STEP_KEYS[step.step] ? t(STEP_KEYS[step.step]) : step.step}
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
            {t("common.cancel")}
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
        <h1 className="mt-5 text-2xl font-semibold">{t("install.doneTitle")}</h1>
        <p className="mt-1 text-muted">{t("install.doneText")}</p>
        <Button className="mt-8" variant="primary" size="xl" onClick={() => void props.onDone(installed)}>
          {t("install.open")}
        </Button>
      </main>
    );
  }

  return (
    <main className="mx-auto flex h-full max-w-2xl flex-col justify-center gap-5 px-8 py-10">
      <div>
        <h1 className="text-2xl font-semibold">{t("install.title")}</h1>
        <p className="mt-1 text-muted">{t("install.subtitle")}</p>
      </div>

      <div>
        <label htmlFor="dest" className="text-sm text-muted">{t("install.installTo")}</label>
        <div className="mt-1 flex gap-2">
          <input
            id="dest"
            value={dest}
            onChange={(e) => setDest(e.target.value)}
            className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2.5 text-[15px] outline-none focus:border-gold"
          />
          <Button onClick={browse}>{t("install.browse")}</Button>
        </div>
      </div>

      {docker && (
        <div>
          <label htmlFor="gamedata" className="text-sm text-muted">{t("install.data.label")}</label>
          <div className="mt-1 flex gap-2">
            <input id="gamedata" value={gameData} onChange={(e) => setGameData(e.target.value)} className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2.5 text-[15px] outline-none focus:border-gold" />
            <Button onClick={browseGameData}>{t("install.browse")}</Button>
          </div>
          <p className="mt-1 text-xs text-muted">{t("install.data.hint")}</p>
          <label htmlFor="linuxpkg" className="mt-4 block text-sm text-muted">{t("install.linuxPkg.label")}</label>
          <div className="mt-1 flex gap-2">
            <input id="linuxpkg" value={pkg} onChange={(e) => setPkg(e.target.value)} placeholder={t("install.linuxPkg.hint")} className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2.5 text-[15px] outline-none focus:border-gold" />
            <Button onClick={browsePackage}>{t("install.browse")}</Button>
          </div>
        </div>
      )}

      {docker && env?.docker_problem && (
        <Card className="border-warn/40 p-4" role="alert">
          <p className="font-medium text-warn">{t("install.docker.title")}</p>
          <p className="mt-1 text-sm text-muted">{t("install.docker.text")}</p>
          <pre className="mt-2 max-h-24 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-2 text-xs">{env.docker_problem}</pre>
          <Button className="mt-3" size="sm" onClick={() => loadEnvironment(false)}>{t("install.docker.retry")}</Button>
        </Card>
      )}

      {pre && pre.problems.some((p) => !(docker && !gameData.trim() && p.code.startsWith("data"))) && (
        <Card className="border-warn/40 p-4" role="alert">
          {pre.problems.filter((p) => !(docker && !gameData.trim() && p.code.startsWith("data"))).map((p) => (
            <p key={p.code} className="text-sm text-warn">{p.message}</p>
          ))}
        </Card>
      )}
      {req && (
        <p className="text-sm text-muted">
          {t("install.size", { size: formatBytes(req.unpacked_bytes), download: formatBytes(req.download_bytes), peak: formatBytes(req.unpacked_bytes + req.download_bytes) })}
        </p>
      )}
      {pre?.ok && <p className="text-sm text-ok">{t("install.ready", { gb: Math.round(pre.free_bytes / (1 << 30)) })}</p>}

      {error && (
        <Card className="border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{error.human.code === "unknown" ? t("install.failedTitle") : human(error.human).title}</p>
          <p className="mt-1 text-sm text-muted">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>
          <p className="mt-1 text-sm text-muted">{t("install.nothingInstalled")}</p>
          <div className="mt-2 flex items-center gap-4">
            <button className="cursor-pointer text-xs text-muted underline" onClick={() => setShowDetails((v) => !v)}>
              {showDetails ? t("install.hideTech") : t("install.showTech")}
            </button>
            <button className="cursor-pointer text-xs text-muted underline" onClick={() => void navigator.clipboard.writeText(error.technical)}>
              {t("common.copyError")}
            </button>
          </div>
          {showDetails && <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-2 text-xs">{error.technical}</pre>}
        </Card>
      )}

      {!docker && <div>
        <button onClick={() => setShowAdvanced((v) => !v)} aria-expanded={showAdvanced} className="flex cursor-pointer items-center gap-1 text-sm text-muted hover:text-ink">
          {showAdvanced ? <ChevronDown className="h-4 w-4" aria-hidden /> : <ChevronRight className="h-4 w-4" aria-hidden />}
          {t("install.advanced")}
        </button>
        {showAdvanced && (
          <div className="mt-2">
            <label htmlFor="pkg" className="text-sm text-muted">{t("install.localPkg")}</label>
            <div className="mt-1 flex gap-2">
              <input id="pkg" value={pkg} onChange={(e) => setPkg(e.target.value)} placeholder={t("install.localPkgHint")} className="selectable flex-1 rounded-md border border-line bg-card px-3 py-2 text-sm outline-none focus:border-gold" />
              <Button size="sm" onClick={browsePackage}>{t("install.browse")}</Button>
            </div>
          </div>
        )}
      </div>}

      <div className="flex items-center gap-3">
        <Button variant="primary" size="md" disabled={!pre?.ok || (docker && (!!env?.docker_problem || !gameData.trim() || !pkg.trim()))} onClick={() => void install()}>
          {t("install.button")}
        </Button>
        {props.canCancel && (
          <Button variant="ghost" onClick={props.onCancel}>
            {t("common.cancel")}
          </Button>
        )}
      </div>
    </main>
  );
}
