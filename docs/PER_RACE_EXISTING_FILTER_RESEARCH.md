# Existing Hide Races filter: internal research

Inspected 2026-10-27. No installed game files were changed. No game, server, database or desktop
automation was used for this investigation.

## Evidence

The owner supplied screenshots of the creation screen and Hide Races panel. The previously
extracted `Interface/GlueXML/CharacterCreate.lua` from `patch-ZZZF.MPQ` contains this implementation.
SHA-256 of the extracted Lua: `8816ef9b6839652db60da6de7d2a6d09ceed1f80323ca5c8e55271a4d5aacac3`.
This identifies the inspected file, not proven runtime archive precedence.

Line numbers below refer to the original LF-delimited file, ignoring carriage-return bytes.
The game source remains outside Git at `C:/games/source/per-race-evidence/`.

| Function / lines | Confirmed behavior |
| --- | --- |
| `CharacterCreate_LayoutRaceButtons`, 138–185 | Reads hidden entries by `#<button index>`, displayed name or lowercase name; hides and relays out buttons by faction |
| Same function, 173–180 | If no button remains visible, shows every button and clears its hidden marker |
| `CharacterCreateEnumerateRaces`, 1864–1899 | Uses native name/English-name/enabled triples; button indices follow enumeration order |
| `CharacterCreate_Finish`, 2184–2212 | Delegates to `CoACharacterNames.CreateCharacter`; no filter-policy check is present in this function |
| `CHAR_CREATE_RACE_IDS` and `SetRace` wrapper, 3404–3459 | Maps English display names and repeated occurrence order to a `bonusRaceID` originally used for racial text |
| Filter persistence, 4603–4701 | Registers `coaHiddenRaces` and `coaHiddenRaces2`; writes with `SetCVar` and optional C_Config setters/save, all protected by `pcall` |
| `RaceEntries`, 4703 onward | Displayed RaceID comes from `bonusRaceID`, first name-map ID, or finally button index |
| Row click, 4844–4856 | Saves the displayed name and `#<button index>`; the displayed RaceID is not the saved key |
| `SetAllChecked`, 4977–4985 | Builds the same name/index set; does not apply a server policy |
| Save handler, 4988–4995 | Stores draft and relays out buttons; does not explicitly choose a permitted replacement race |
| Row refresh, 5018–5041 | Stores `info.id` (button index) in `row.raceId`, while the label displays `info.raceID` |
| Click wrapper, 5056–5069 | Redirects a click on a hidden button to the first visible one |

The name-to-ID table is hardcoded and extended repeatedly later in the Lua. Examples of paired
names: Pandaren `{29,20}`, Murloc `{30,31}`, Earthen `{68,69}`, Haranir `{71,70}`,
Centaur `{75,102}` and Drakonid `{100,98}`. Name localization and enumeration-order changes
need validation; this table is not a universal verified RaceID API.

## Offline reproduction

An isolated harness executed the actual extracted layout, persistence and click-wrapper functions
using the already installed `lupa.lua51` runtime. Buttons, CVar storage, faction lookup and filter
button creation were mocked. No render, native race API, packet send, account or database was used.

Harness: `C:/games/source/per-race-evidence/probe_existing_filter.py`.
Machine-readable results: `C:/games/source/per-race-evidence/existing-filter-probe.json`.

All nine assertions were confirmed:

1. Saving one Pandaren by name hides both same-named Pandaren buttons.
2. A saved index plus saved name can hide two different races after reordering.
3. Hiding all buttons makes the layout show all buttons again.
4. Saving 128 synthetic index entries persists at most 480 characters; a fresh runtime recovered
   only 115 of those indices. The exact losses depend on the input and sort order.
5. Saving/hiding the currently selected button leaves the harness's selection unchanged.
6. Clicking a hidden button redirects to the first visible one.
7. The same preference values apply across independent Glue runtimes; no realm identifier is read.
8. Saving `#82` does not hide Sethrak at button index 2, even when the button's `bonusRaceID`
   is 82. Current serialization cannot simply be given real IDs instead of indices.
9. If persistent setters fail, saving still updates the in-memory list and reports no failure;
   a fresh runtime loses that list. Protected calls suppress the errors.

The initial harness run lacked a stub for filter-button creation and failed six checks before
their assertions; adding that UI stub allowed the original assertions to run and pass. The later
expanded run passed all nine assertions. This correction
did not modify the inspected game Lua. These results are offline behavior checks, not live acceptance.

## Persistence and identity cross-check

Read-only searches of both the primary client and isolated copy found no `coaHiddenRaces` or
`coaHiddenRaces2` entries in their `WTF/Config.wtf` or `WTF/Custom/GlueConfig.json`, or in the
searched text configuration files. There is therefore no observed on-disk value establishing
where this installation persists the list. The presence of `SetCVar` calls alone does not prove
that an arbitrary registered CVar is archived. C_Config setters/save are optional and their
failures are suppressed. Physical persistence must be verified later by a controlled save/exit
and file comparison, when the owner allows game testing. The earlier Config.wtf suggestion
remains a hypothesis, not an established path.

Listfile-guided scanning of Lua members in patch-B, patch-T and patch-ZZZF found the filter's
setter calls but no implementation of these C_Config persistence methods. This does not establish
their absence at runtime: native code or another archive may provide them. No native client code
was executed or patched to investigate this.

The final `CHAR_CREATE_RACE_IDS` map was reconstructed by executing its literal declarations and
subsequent assignments in Lua 5.1. All 73 inspected ChrRaces rows whose NOT_PLAYABLE bit is clear
are represented by their English name and actual ID in that map. The comparison includes final
overrides, rather than only the initial table. Result:
`C:/games/source/per-race-evidence/race-filter-id-map-audit.json`.

This is good evidence that the map can support this version's UI integration. It is not proof
that each runtime button is assigned the right ID: repeated names depend on occurrence order,
the engine may enumerate a different set/order, and localized labels may not match the English
keys. `bonusRaceID` remains a candidate identity, not an authoritative native lookup.

## Suitability for CSM

The existing layout and UI are useful foundations. Directly replacing `coaHiddenRaces` with owner
restrictions is unsuitable: the player can clear the list, names can collide, indices can shift,
serialization truncates and the preferences are not realm-scoped. It would also overwrite the
player's own display choices. A local filter cannot enforce server creation policy.

Recommended integration to investigate next:

- Keep personal Hide Races preferences independent from the server-managed disabled RaceID set.
- Resolve actual RaceID per button from validated version-specific client data and faction/identity.
  Never use a button-index fallback as an authoritative RaceID. Ambiguous mapping is unsupported,
  not permission to hide another race with the same name.
- Compute visibility from personal preference OR the server restriction. Check All may clear
  personal preferences but must not remove the server restriction.
- Preserve stock-race behavior and the existing layout, class-selection and Esteria wrappers.
  The empty-layout fallback must not reveal a server-prohibited race.
- Validate selected race before advancing/creating, including after save, randomization, sex
  changes and native selection resets. Do not renumber native race indices.
- Reconcile the selected local realm before PLAY with the client closed. Do not guess public-server
  policy or mutate the player's saved preference when switching realms.
- Independently enforce creation on the server and in new-bot factories, preserving existing
  character login/import and existing-bot admission.

One possible delivery is a small GlueXML extension in the single managed overlay, reusing existing
layout entry points while supplying a separately generated policy table. This is a proposal, not
an existing API or a selected filename. It still requires proven loading order and live validation.
An alternative CVar-based channel needs its own persistence and truncation checks and must not
reuse the personal hidden-list keys.

## Decision

Prioritize investigation of the existing GlueXML mechanism as the client integration candidate.
Do not yet commit to a DBC-only overlay, and do not declare the DBC hypothesis failed: it has not
been live-tested. No gameplay/client support claim follows from these screenshots or mock tests.

The next live acceptance must establish correct real RaceID mapping, one disabled race, independent
faction variants, selected-race replacement, Check All resistance to owner-policy removal, ordinary
race creation, existing-character login and profile switching. Interactive/database tests remain
deferred at the owner's request.

## User-operated visibility experiment and disabled-row prototype

The owner subsequently authorized a temporary patch in the isolated client. The first probe added
only a real-ID predicate for Sethrak 82, Gnoll 83 and Saberon 97 to the existing layout. A single
`Data/patch-ZZZZZ.MPQ` contained the modified CharacterCreate.lua; original archives and DBCs
were unchanged. The owner reported the races disappeared and supplied screenshots consistent
with that result. This is evidence that this particular Lua overlay affects this installation's
creation screen, not proof of universal MPQ precedence or completed creation/login tests.

The owner then selected the UI behavior: hide owner-prohibited races on the creation screen;
show them grey, unchecked and disabled in Hide Races, with a server-disabled label. Personal
preferences remain independent, including when the owner later re-enables the race.

`tools/race_visibility_lua_poc.py` now generates a version-pinned second laboratory probe. It
accepts only the exact inspected Lua fingerprint and the three previously tested RaceIDs.
Unknown identity is not interpreted as a button index. It guards disabled row clicks, restores
enabled state when a scroll row is reused, and prevents the empty-layout fallback from revealing
an owner-prohibited race. The disabled label has EN/RU/DE/FR/ES/ZH translations.

Eleven tests passed under Lua 5.1, covering syntax, disabled row state, personal preference,
scroll-row reuse, ambiguous identity, Check All, direct disabled-row click, six translations,
unsupported input rejection, empty-layout fallback and lifting restrictions.

Probe v2 was MPQ-roundtrip verified and installed only after observing that the isolated client
was closed. The previous owned probe's SHA-256 was checked before replacement, the existing
client patch lock was held, and the prior bytes were preserved outside the client. Its evidence
is `C:/games/source/per-race-evidence/lua-visibility-probe-v2/report.json`.
The primary client, all databases and all server processes were untouched.

This prototype still contains a fixed laboratory restriction set, not the future server-driven
policy. The owner confirmed grey, unchecked rows for the three probe races in game and supplied
screenshots. Selected-race validation, realm policy
reconciliation, source update handling, server enforcement and new-bot integration remain work.
