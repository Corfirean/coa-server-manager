import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { api, asUiError, type CompanionSizes, type Population, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { hasKey, useHuman, useT, type Key } from "@/i18n";

export function CompanionsCard({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [sizes, setSizes] = useState<CompanionSizes | null>(null);
  const [pop, setPop] = useState<Population | null>(null);
  const [running, setRunning] = useState(false);
  const [choice, setChoice] = useState<string>("small");
  const [custom, setCustom] = useState("100");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    void api.companionSizes().then(setSizes);
    let alive = true;
    const poll = async () => {
      const s = await api.status(serverId).catch(() => null);
      if (!alive || !s) return;
      const up = s.observed.world.state === "running";
      setRunning(up);
      setPop(up ? await api.population(serverId).catch(() => null) : null);
    };
    void poll();
    const t = setInterval(poll, 4000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [serverId]);

  // The backend describes a size's warning in English; show the translated text for the two known kinds.
  const warningFor = (size?: { bots: number; warning: string | null }) => {
    if (!size?.warning) return null;
    return size.warning.startsWith(`${size.bots} companions`)
      ? t("comp.warnHardware", { n: size.bots, cores: sizes?.hardware.cores ?? "?", ram: sizes ? sizes.hardware.ram_gb.toFixed(0) : "?" })
      : t("comp.warnMemory");
  };
  const count = choice === "custom" ? Number(custom) : (sizes?.sizes.find((s) => s.id === choice)?.bots ?? 0);
  const warning = choice === "custom" ? (count > 500 ? t("comp.largeWarning") : null) : (warningFor(sizes?.sizes.find((s) => s.id === choice)));
  const valid = Number.isInteger(count) && count >= 1 && count <= 2000;

  async function add() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const r = await api.addCompanions(serverId, count);
      setNote(r.spawned ? t("comp.spawned", { n: count }) : t("comp.saved", { n: count }));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">{t("comp.title")}</h2>
      <p className="mt-1 text-sm text-muted">
        {pop
          ? t("comp.population", { online: pop.bots_online, total: pop.bots_total })
          : running
            ? t("comp.reading")
            : t("comp.startToSee")}
      </p>

      <fieldset className="mt-4">
        <legend className="text-sm text-muted">{t("comp.wantMore")}</legend>
        <div className="mt-2 flex flex-wrap gap-2">
          {sizes?.sizes.map((s) => (
            <label key={s.id} className={`cursor-pointer rounded-md border px-3 py-2 text-sm ${choice === s.id ? "border-gold bg-gold/10" : "border-line hover:bg-white/5"}`}>
              <input type="radio" name="size" className="sr-only" checked={choice === s.id} onChange={() => setChoice(s.id)} />
              {hasKey(`comp.size.${s.id}`) ? t(`comp.size.${s.id}` as Key) : s.title} <span className="text-muted">· {s.bots}</span>
            </label>
          ))}
          <label className={`cursor-pointer rounded-md border px-3 py-2 text-sm ${choice === "custom" ? "border-gold bg-gold/10" : "border-line hover:bg-white/5"}`}>
            <input type="radio" name="size" className="sr-only" checked={choice === "custom"} onChange={() => setChoice("custom")} />
            {t("comp.custom")}
          </label>
          {choice === "custom" && (
            <input aria-label={t("comp.numberLabel")} value={custom} onChange={(e) => setCustom(e.target.value)} inputMode="numeric" className="w-24 rounded-md border border-line bg-bg px-3 py-2 text-right text-sm outline-none focus:border-gold" />
          )}
        </div>
      </fieldset>

      {warning && <p className="mt-3 text-sm text-warn" role="status">{warning}</p>}
      {sizes && <p className="mt-2 text-xs text-muted">{t("comp.hardware", { cores: sizes.hardware.cores, ram: sizes.hardware.ram_gb.toFixed(0) })}</p>}

      <div className="mt-4 flex items-center gap-3">
        <Button variant="primary" disabled={busy || !valid} onClick={() => void add()}>
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {valid ? t("comp.add", { n: count }) : t("comp.addPlain")}
        </Button>
        {!running && <span className="text-xs text-muted">{t("comp.stoppedHint")}</span>}
      </div>
      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </Card>
  );
}
