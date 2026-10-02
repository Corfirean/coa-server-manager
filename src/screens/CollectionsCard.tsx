import { useCallback, useEffect, useState } from "react";
import { api, asUiError, type ModuleSetting, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Card } from "@/components/ui/card";
import { useHuman, useT, type Key } from "@/i18n";

/** The collection and convenience switches of the client compatibility part (the same file holds them under either name). */
const SWITCHES: { name: string; label: Key; text: Key }[] = [
  { name: "UnlockLocalAppearanceCatalog", label: "col.appearances", text: "col.appearances.text" },
  { name: "UnlockAllVanity", label: "col.vanity", text: "col.vanity.text" },
  { name: "AutoCollectAppearances", label: "col.auto", text: "col.auto.text" },
  { name: "LearnOwnedCompanions", label: "col.companions", text: "col.companions.text" },
  { name: "MaxRidingFromStart", label: "col.riding", text: "col.riding.text" },
];

/** Appearances (transmog), vanity and a few solo conveniences: plain switches for what lives in the compatibility file. */
export function CollectionsCard({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [items, setItems] = useState<ModuleSetting[] | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [changed, setChanged] = useState(false);

  const load = useCallback(async () => {
    try {
      setItems(await api.moduleSettings(serverId, "client-compat"));
    } catch {
      setItems([]); // an older build without the file: nothing to offer
    }
  }, [serverId]);

  useEffect(() => {
    void load();
  }, [load]);

  const find = (name: string) => items?.find((i) => i.key.endsWith("." + name));
  const shown = SWITCHES.filter((s) => find(s.name));
  if (!items || shown.length === 0) return null;

  async function toggle(name: string) {
    const item = find(name);
    if (!item) return;
    setError(null);
    try {
      await api.moduleSaveSettings(serverId, "client-compat", { [item.key]: item.value.trim() === "1" ? "0" : "1" });
      setChanged(true);
      await load();
    } catch (e) {
      setError(asUiError(e));
    }
  }

  return (
    <Card className="mt-8 p-5">
      <h2 className="font-semibold">{t("col.title")}</h2>
      <p className="mt-1 text-sm text-muted">{t("col.intro")}</p>
      <ul className="mt-3 divide-y divide-line">
        {shown.map((s) => {
          const on = find(s.name)?.value.trim() === "1";
          return (
            <li key={s.name} className="flex items-center justify-between gap-4 py-3">
              <div className="min-w-0">
                <p className="text-sm font-medium">{t(s.label)}</p>
                <p className="mt-0.5 text-xs text-muted">{t(s.text)}</p>
              </div>
              <button
                role="switch"
                aria-checked={on}
                aria-label={t(s.label)}
                onClick={() => void toggle(s.name)}
                className={cn("relative h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors", on ? "bg-gold" : "bg-white/15")}
              >
                <span className={cn("absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all", on ? "left-[22px]" : "left-0.5")} />
              </button>
            </li>
          );
        })}
      </ul>
      {changed && <p className="mt-3 text-sm text-muted" role="status">{t("mod.restart")}</p>}
      {error && <p className="mt-2 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </Card>
  );
}
