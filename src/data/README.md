# Release history data (internal)

The Manager feed is imported from the canonical `changelog/history.json`. The
server snapshot in this directory is copied from `Corfirean/coa-server-build`
commit `15687243bd3f4d60dfedf70b938b6076c1bd6393`, path
`changelog/history.json`, on 2026-10-30. Only schema, component, repository and
releases are retained. Do not independently edit historical records here.

At runtime the native read-only command fetches the fixed canonical URL, bounds
and validates the feed, and caches it for offline use. The bundled records appear
immediately and remain available if neither network nor cache is usable. These
records cannot select a package or authorize an update.

`latest-note-translations.json` contains the reviewed Manager notes through 0.9.3, the new
0.9.4 Manager entries, and the 0.261028.83 server notes in Russian, German, French, Spanish and Simplified
Chinese. English remains canonical. Translations are keyed by exact original
text: changing an original invalidates its translation. A partly translated
release falls back entirely to English and displays an explicit notice.

When preparing future release notes, refresh the server snapshot from a pinned
canonical commit and add all five translations for each new original note.
Never rewrite released fragments to add translations. Run
`node --test tools/release-history.test.mjs`; its completeness check expects the
latest bundled record of each component to have every translation.

Manager 0.9.5 prepares all five translations of the ten reviewed stabilization notes and
three server stabilization notes before their release. The bundled snapshot includes server
0.261029.85 from the pinned commit above. Pending notes do not fabricate published records.
