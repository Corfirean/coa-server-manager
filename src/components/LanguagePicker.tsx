import { Globe } from "lucide-react";
import { LOCALES, useI18n, type Locale } from "@/i18n";

/** Language switch shared by the welcome screen and Settings. Names are shown in their own language. */
export function LanguagePicker({ className }: { className?: string }) {
  const { t, locale, setLocale } = useI18n();
  return (
    <label className={`inline-flex items-center gap-2 text-sm text-muted ${className ?? ""}`}>
      <Globe className="h-4 w-4" aria-hidden />
      <span className="sr-only">{t("settings.language")}</span>
      <select
        value={locale}
        onChange={(e) => setLocale(e.target.value as Locale)}
        className="cursor-pointer rounded-md border border-line bg-bg px-3 py-2 text-sm text-ink outline-none focus:border-gold"
      >
        {LOCALES.map((l) => (
          <option key={l.id} value={l.id}>
            {l.name}
          </option>
        ))}
      </select>
    </label>
  );
}
