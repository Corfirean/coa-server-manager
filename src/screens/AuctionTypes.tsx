import { useRef } from "react";
import { useI18n } from "@/i18n";
import type { ModuleSetting } from "@/lib/api";
import { auctionCategories, auctionCategoryLabel, auctionCategorySettings, auctionQualityLabel, auctionTypeChanges, auctionWeightKey, canonicalModuleValue } from "@/lib/moduleSettings";
import { cn } from "@/lib/utils";

export function AuctionTypes({ items, edits, busy, query, errors, onChange }: {
  items: ModuleSetting[]; edits: Record<string, string>; busy: boolean; query: string;
  onChange: (changes: Record<string, string>) => void;
  errors: Record<string, string>;
}) {
  const { t, locale } = useI18n();
  const remembered = useRef<Record<string, Record<string, string>>>({});
  const groups = Object.keys(auctionCategories).map(category => ({ category, settings: auctionCategorySettings(items, category) })).filter(group => group.settings.length);
  if (!groups.length) return null;
  const valueOf = (item: ModuleSetting) => canonicalModuleValue(item.key, edits[item.key] ?? item.value);
  const matching = groups.filter(group => `${auctionCategoryLabel(group.category, locale)} ${t("mod.ahbot.types")} ${group.settings.map(s => `${s.key} ${auctionQualityLabel(auctionWeightKey(s.key)![2], locale)}`).join(" ")}`.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale)));
  if (!matching.length) return null;
  return <fieldset className="my-4 rounded-md border border-line px-4 pb-2">
    <legend className="px-1 text-sm font-semibold">{t("mod.ahbot.types")}</legend>
    <p className="mb-2 mt-1 text-sm text-muted">{t("mod.ahbot.typesHint")}</p>
    <div className="grid gap-x-6 sm:grid-cols-2">
      {matching.map(({ category, settings }) => {
        const on = settings.some(item => Number(valueOf(item)) > 0);
        const label = auctionCategoryLabel(category, locale);
        return <div key={category} className="border-t border-line py-3">
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm font-medium">{label}{settings.some(s => s.key in edits) && <span className="ml-2 text-xs text-gold">{t("set.changed")}</span>}</span>
            <button role="switch" aria-label={label} aria-checked={on} disabled={busy} className={cn("relative h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors", on ? "bg-gold" : "bg-white/15")} onClick={() => {
              if (on) remembered.current[category] = Object.fromEntries(settings.map(item => [item.key, valueOf(item)]));
              const original = Object.fromEntries(settings.map(item => [item.key, canonicalModuleValue(item.key, item.value)]));
              const saved = Object.values(original).some(v => Number(v) > 0) ? original : undefined;
              onChange(auctionTypeChanges(settings, !on, remembered.current[category] ?? saved));
            }}><span className={cn("absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white transition-transform", on && "translate-x-5")} /></button>
          </div>
          <details className="mt-2 text-xs text-muted">
            <summary className="cursor-pointer">{t("mod.ahbot.qualityWeights")}</summary>
            <p className="my-2 leading-relaxed">{t("mod.ahbot.weightsHint")}</p>
            {settings.map(item => <label key={item.key} className="mb-1 flex items-center justify-between gap-2">
              {auctionQualityLabel(auctionWeightKey(item.key)![2], locale)}
              <input aria-label={`${label}: ${auctionQualityLabel(auctionWeightKey(item.key)![2], locale)}`} type="number" min={0} max={1000} step={1} value={edits[item.key] ?? item.value} disabled={busy} onChange={e => onChange({ [item.key]: e.target.value })} className="w-20 rounded border border-line bg-bg px-2 py-1 text-sm text-ink outline-none focus:border-gold" />
            </label>)}
          </details>
          {settings.filter(item => errors[item.key]).map(item => <p key={item.key} role="alert" className="mt-1 text-xs text-bad">{auctionQualityLabel(auctionWeightKey(item.key)![2], locale)}: {errors[item.key]}</p>)}
        </div>;
      })}
    </div>
    {!groups.some(group => group.settings.some(item => Number(valueOf(item)) > 0)) && <p className="my-2 text-sm text-warn" role="status">{t("mod.ahbot.noTypes")}</p>}
  </fieldset>;
}
