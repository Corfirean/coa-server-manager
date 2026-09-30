import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import en from "./locales/en";
import ru from "./locales/ru";
import de from "./locales/de";
import fr from "./locales/fr";
import es from "./locales/es";
import ruSchema from "./schema/ru.json";
import deSchema from "./schema/de.json";
import frSchema from "./schema/fr.json";
import esSchema from "./schema/es.json";

export type Locale = "en" | "ru" | "de" | "fr" | "es";
export type Key = keyof typeof en;
type Vars = Record<string, string | number>;

export const LOCALES: { id: Locale; name: string }[] = [
  { id: "en", name: "English" },
  { id: "ru", name: "Русский" },
  { id: "de", name: "Deutsch" },
  { id: "fr", name: "Français" },
  { id: "es", name: "Español" },
];

const DICTS: Record<Locale, Partial<Record<Key, string>>> = { en, ru, de, fr, es };
const STORAGE_KEY = "coa-locale";

function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY) as Locale | null;
    if (saved && saved in DICTS) return saved;
  } catch {
    /* storage may be unavailable */
  }
  const guess = (navigator.language || "en").slice(0, 2).toLowerCase();
  return (guess in DICTS ? guess : "en") as Locale;
}

/** Replace `{name}` placeholders. Unknown placeholders are left visible so a missing variable is noticed. */
export function interpolate(text: string, vars?: Vars): string {
  return vars ? text.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m)) : text;
}

/** Look a key up in `locale`, falling back to English, then to the key itself. */
export function lookup(locale: Locale, key: Key, vars?: Vars): string {
  return interpolate(DICTS[locale][key] ?? en[key] ?? key, vars);
}

/** Plural form: tries `key_<category>` (one/few/many/other) for the locale, then `key_other`, then `key`. */
export function lookupPlural(locale: Locale, key: string, n: number, vars?: Vars): string {
  const cat = new Intl.PluralRules(locale).select(n);
  const d = DICTS[locale];
  const text = d[`${key}_${cat}` as Key] ?? d[`${key}_other` as Key] ?? en[`${key}_${cat}` as Key] ?? en[`${key}_other` as Key] ?? en[key as Key] ?? key;
  return interpolate(text, { n, ...vars });
}

interface Ctx {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: Key, vars?: Vars) => string;
  tn: (key: string, n: number, vars?: Vars) => string;
}

const I18n = createContext<Ctx>({
  locale: "en",
  setLocale: () => {},
  t: (k, v) => lookup("en", k, v),
  tn: (k, n, v) => lookupPlural("en", k, n, v),
});

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(initialLocale);
  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    try {
      localStorage.setItem(STORAGE_KEY, l);
    } catch {
      /* ignore */
    }
    document.documentElement.lang = l;
  }, []);
  const value = useMemo<Ctx>(
    () => ({ locale, setLocale, t: (k, v) => lookup(locale, k, v), tn: (k, n, v) => lookupPlural(locale, k, n, v) }),
    [locale, setLocale],
  );
  return <I18n.Provider value={value}>{children}</I18n.Provider>;
}

export const useI18n = () => useContext(I18n);

/** Translated text of the settings schemas (titles, descriptions, categories, presets, units). English comes from the backend. */
interface SchemaOverlay {
  categories: Record<string, string>;
  units: Record<string, string>;
  settings: Record<string, { t: string; d: string }>;
  options: Record<string, string>;
  presets: Record<string, { t: string; d: string }>;
}
const SCHEMA: Partial<Record<Locale, SchemaOverlay>> = { ru: ruSchema, de: deSchema, fr: frSchema, es: esSchema };

export function useSchemaText() {
  const { locale } = useContext(I18n);
  const o = SCHEMA[locale];
  return {
    title: (s: { key: string; title: string }) => o?.settings[s.key]?.t ?? s.title,
    description: (s: { key: string; description: string }) => o?.settings[s.key]?.d ?? s.description,
    category: (scope: string, c: { id: string; title: string }) => o?.categories[`${scope}.${c.id}`] ?? c.title,
    option: (key: string, opt: { value: unknown; label: string }) => o?.options[`${key}.${String(opt.value)}`] ?? opt.label,
    unit: (u: string) => o?.units[u] ?? u,
    preset: (p: { id: string; title: string; description: string }) => ({ title: o?.presets[p.id]?.t ?? p.title, description: o?.presets[p.id]?.d ?? p.description }),
  };
}

/** Translated title/message for a backend error; falls back to the backend's English text for unknown codes. */
export function useHuman() {
  const { t } = useContext(I18n);
  return (h: { code: string; title: string; message: string }) => {
    const tk = `err.${h.code}.title` as Key;
    const mk = `err.${h.code}.message` as Key;
    return { title: tk in en ? t(tk) : h.title, message: mk in en ? t(mk) : h.message };
  };
}
export const useT = () => useContext(I18n).t;

/** True when `key` exists in the catalogue (backend-supplied texts are translated only when a key is defined for them). */
export const hasKey = (key: string): key is Key => key in en;
