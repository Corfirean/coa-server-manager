import { RotateCcw } from "lucide-react";
import type { AllSetting, ModuleSetting } from "@/lib/api";
import { cn } from "@/lib/utils";
import { useT, type Key } from "@/i18n";

/** The collection and convenience switches of the client compatibility part (the same file holds them under either name). */
const SWITCHES: { name: string; label: Key; text: Key }[] = [
  { name: "UnlockLocalAppearanceCatalog", label: "col.appearances", text: "col.appearances.text" },
  { name: "UnlockAllVanity", label: "col.vanity", text: "col.vanity.text" },
  { name: "AutoCollectAppearances", label: "col.auto", text: "col.auto.text" },
  { name: "LearnOwnedCompanions", label: "col.companions", text: "col.companions.text" },
  { name: "MaxRidingFromStart", label: "col.riding", text: "col.riding.text" },
];

export interface Collection {
  item: ModuleSetting;
  label: Key;
  text: Key;
}

/** The switches the compatibility file actually has; an older build without the file offers none. */
export function collectionsOf(items: ModuleSetting[]): Collection[] {
  return SWITCHES.flatMap((s) => {
    const item = items.find((i) => i.key.endsWith("." + s.name));
    return item ? [{ item, label: s.label, text: s.text }] : [];
  });
}

const ROW = "grid grid-cols-[1fr_auto] items-start gap-x-6 gap-y-1 py-3.5";

/** Appearances (transmog), vanity and a few solo conveniences, in the look of every other setting row. */
export function CollectionRows(props: { items: Collection[]; edits: Record<string, string>; onChange: (key: string, value: string | null) => void }) {
  const t = useT();
  return (
    <div className="divide-y divide-line">
      {props.items.map(({ item, label, text }) => {
        const value = props.edits[item.key] ?? item.value;
        const on = value.trim() === "1";
        return (
          <div key={item.key} className={ROW}>
            <div>
              <label htmlFor={`col-${item.key}`} className="flex flex-wrap items-center gap-2 font-medium">
                {t(label)}
                {item.key in props.edits && <span className="rounded bg-gold/15 px-1.5 py-0.5 text-[11px] font-normal text-gold">{t("set.changed")}</span>}
              </label>
              <p className="mt-0.5 max-w-xl text-sm text-muted">{t(text)}</p>
            </div>
            <div className="pt-0.5">
              <button
                id={`col-${item.key}`}
                role="switch"
                aria-checked={on}
                onClick={() => {
                  const next = on ? "0" : "1";
                  props.onChange(item.key, next === item.value.trim() ? null : next);
                }}
                className={cn("relative h-7 w-14 cursor-pointer rounded-full transition-colors", on ? "bg-gold" : "bg-white/15")}
              >
                <span className={cn("absolute left-1 top-1 h-5 w-5 rounded-full bg-white transition-transform", on && "translate-x-7")} />
                <span className="sr-only">{on ? t("set.on") : t("set.off")}</span>
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** Documented world server settings that have no friendlier control: the name, its explanation, a text box. */
export function RawRows(props: { items: AllSetting[]; edits: Record<string, string>; onChange: (key: string, value: string | null) => void }) {
  const t = useT();
  return (
    <div className="divide-y divide-line">
      {props.items.map((s) => {
        const value = props.edits[s.key] ?? s.value;
        const set = (v: string) => props.onChange(s.key, v === s.value ? null : v);
        return (
          <div key={s.key} className={ROW}>
            <div className="min-w-0">
              <label htmlFor={`all-${s.key}`} className="selectable flex flex-wrap items-center gap-2 font-medium">
                {s.key}
                {(s.changed || s.key in props.edits) && <span className="rounded bg-gold/15 px-1.5 py-0.5 text-[11px] font-normal text-gold">{t("set.changed")}</span>}
              </label>
              <p className="selectable mt-0.5 max-w-xl text-sm text-muted">{s.doc}</p>
              <p className="mt-1 text-xs text-muted/80">{t("set.default", { v: s.default })}</p>
            </div>
            <div className="flex items-center gap-2 pt-0.5">
              <input
                id={`all-${s.key}`}
                value={value}
                onChange={(e) => set(e.target.value)}
                className="selectable w-56 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold"
              />
              <button
                title={t("set.reset")}
                aria-label={t("set.resetItem", { name: s.key })}
                onClick={() => set(s.default)}
                disabled={value === s.default}
                className="cursor-pointer rounded p-1.5 text-muted hover:bg-white/5 hover:text-ink disabled:opacity-25"
              >
                <RotateCcw className="h-4 w-4" aria-hidden />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
