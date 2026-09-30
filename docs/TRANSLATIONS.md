# Translations

The Manager interface exists in English (source), Russian, German, French and Spanish. Everything except English is a
**draft written by an AI assistant and needs review by a native speaker** before it is treated as final.

## Where the texts are

| What | File | Notes |
|---|---|---|
| Interface texts | `src/i18n/locales/<lang>.ts` | flat `"key": "text"` lists; `en.ts` is the source of truth, other languages fall back to it |
| Setting titles, descriptions, categories, presets, units, option labels | `src/i18n/schema/<lang>.json` | generated once from `schemas/*.json`; edit the JSON directly |
| Backend error messages | keys `err.<code>.title` / `err.<code>.message` in the locale files | codes come from `crates/coa-core/src/error.rs` |

`{name}` placeholders must stay exactly as they are. Plural forms use the suffixes `_one`, `_few`, `_many`, `_other`
(the browser picks the right one for the language).

## Checking your work

```
node tools/check-i18n.mjs      # unknown keys, changed placeholders, coverage, schema overlays complete
npx tsc --noEmit               # types
npx vite                       # UI with mock data in a browser; pick the language on the welcome screen
```

## Adding a language

1. Copy `src/i18n/locales/en.ts` to `<lang>.ts` (keep the keys), translate, and register it in `src/i18n/index.tsx`
   (`Locale`, `LOCALES`, `DICTS`).
2. Copy `src/i18n/schema/ru.json` to `<lang>.json`, translate the values, and register it in `SCHEMA`.
3. Run the check above.

## Known gaps

* Free-form technical messages produced deep inside the backend (`Error::Invalid("...")`) stay in English; they are
  shown as "technical details".
* Texts inside the game itself (client, server data) are not part of the Manager.
