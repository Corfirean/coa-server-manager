# Portable Characters - Phase 0 audit

Status: **Phase 0 output. No production code was written.** Everything marked *fact* was read from the real code or
from a real database schema; everything marked *proposal* or *decision* is a recommendation that needs the owner's
sign-off before the phase that depends on it.

Scope of the audit: the Manager (`Corfirean/coa-server-manager`) and the core
(`azerothcore-wotlk-coa`, characters database and the C++ that reads and writes it). The bots module was only checked
for what it puts into the characters database.

---------------------------------------------------------------------------------------------------------

## 0. Method and evidence base

| Source | What was used |
|---|---|
| Manager | `origin/master` at `cd3a963` (v0.6.4), read in a clean worktree. Baseline `cargo test --workspace --locked`: **242 tests, all green** |
| Core | local checkout `coa-core-fork`, branch `codex/squid-disabled-startup`, commit `641251c4f` (2026-10-05). Line numbers below refer to this commit and will drift |
| Characters schema | **real**, not inferred from SQL files: a disposable fixture database (`coa-schema-fixture-20261005`, copied and opened read-only on a private MySQL instance with no network port; the owner's live database was never touched). 163 tables |
| Real data shape | a second disposable fixture (`coa-fixture`, 1861 characters, 1815 of them bots, 46 on non-bot accounts) for formats and sizes. Only aggregate numbers appear in this document |

Caveats that matter for the conclusions:

* Three core revisions are in circulation on the owner's machine (the Manager's own architecture notes say the same).
  Every compatibility decision below must be keyed by an **exact core commit / schema contract**, never by "latest".
* The `coa-fixture` data is older (core 0.3.0) than the schema fixture, so it has no `core.ascension_build.*` /
  `core.wildcard.*` settings rows. Those sources are known from the *code*, not from sample data.
* All of the real characters in the fixtures are level 80 (45 of 46) or 21 (1), so level-cap behaviour was audited from
  code only.

---------------------------------------------------------------------------------------------------------

## 1. Executive summary

**Verdict: no architectural contradiction was found that blocks Phase 1** (local canonical storage), and the model in
the task description holds. But the audit found seven facts that change *how* Phases 2-8 must be built. Each is
presented as a decision in section 14; the first three need an answer **before Phase 3**.

1. **CoA's character state is not in the stock AzerothCore tables.** CoA has 21 classes (class ids 12-32; no stock
   classes in the real data) and keeps talents/builds/spec bars in `character_spell` plus **`character_settings`**
   (sources `core.ascension_*`, `core.wildcard.*`, ...). `character_talent` has **0 rows** in real data. The stock
   `.pdump` tool does not know `character_settings`, the wardrobe tables or any `coa_*` table, so it would silently
   export a character with no build, no spec, no spec bars.
2. **Writing into a *running* realm's characters database is unsafe.** The worldserver hands out character, item and
   pet ids from counters it reads once at startup, and keeps a name cache it never re-reads. An external importer that
   inserts rows while the realm is up can collide with ids the server is about to hand out, and the imported
   character is invisible to name lookups until a restart. The core's own `.pdump load` solves exactly this *inside*
   the server. Options and a recommendation are in section 10.
3. **The database is stale during play.** Characters are persisted on an autosave timer, on logout and on a few
   events, not every 60 seconds. There is no console command that saves **one** character (`.save` is player-only,
   `.saveall` saves every online player, bots included). The 60-second checkpoint of Phase 5 therefore needs a small
   core change.
4. **Loading a character is destructive.** The server deletes items whose entry it does not know, conjured items after
   15 minutes offline, out-of-season holiday items, expired refund data, and **mails** items it cannot equip. So a
   realm's *export* is not the character; applying "replace canonical with export" would permanently lose anything the
   destination had to filter. Export must be reconciled against the canonical snapshot per item (section 8.4).
5. **Level-cap projection (Phase 8) cannot be done by rewriting rows.** An item above the cap's required level is
   *mailed away and removed from the inventory table* on load. Phase 8 needs core-side hooks, not just a projected
   snapshot.
6. **Collections are account-wide, keyed by the destination's account id, loaded into memory at login, and strictly
   additive** (`INSERT IGNORE` only, no `DELETE` anywhere in the code). Union semantics are correct and safe. They are
   small (hundreds to low thousands of ids), not tens of thousands.
7. **The Manager has no database driver.** Every query is a string passed to the bundled `mysql` command-line client
   (also inside a Docker container for Docker installs). That is acceptable for Manager-authored queries but unsafe for
   importing data that came from another user's Manager (SQL injection through names, settings, text). Phases 3 and
   15 should use a real driver with prepared statements.

Things that came out **better** than feared:

* One snapshot is small: a real level-80 character has about 1,300 spells, 105 reputations, 23 skills and at most ~40
  inventory rows. Raw data is tens of KB; compressed, a few KB. A 60-second checkpoint is cheap.
* The database engine is InnoDB for every relevant table (3 MyISAM tables are bot name lists), so multi-table
  transactions work. There are only 4 foreign keys, none involving characters or items, and no triggers or stored
  procedures, so import order is not constrained by the schema.
* Item state is compact and fully contained in `item_instance` (no hidden per-item side tables except gifts, refund
  and BoP-trade windows).

---------------------------------------------------------------------------------------------------------

## 2. Rulesets and class model (fact)

* Real data: classes **12-32** (21 classes, none of the stock 1-11), races 1-8, 10, 11 (no 9). Class and race ids are
  therefore *CoA-specific numbers* and must be stored under a namespace, not as bare "WoW class ids"
  (`coa:class:12` rather than `class=12`).
* The Manager already has two **realm profiles** (`crates/coa-core/src/realms.rs:15-40`): `Coa` (realm id 1, schemas
  `acore_characters` / `acore_world`) and `Wildcard` (realm id 2, `acore_characters_wildcard` /
  `acore_world_wildcard`), sharing one `acore_auth`. Wildcard has its own rules (skill cards, `core.wildcard.*`
  settings, account-bound season state). **A portable character must carry its `ruleset`**, and CoA <-> Wildcard
  transfer should be refused unless the owner decides otherwise (decision D5).
* Content is global within a ruleset: item ids such as 2977351 (welcome warchest) and spell ids >= 500000 are the same
  on every CoA realm that uses the same content pack. That is what makes numeric ids meaningful *inside* a ruleset;
  across rulesets or modded realms they are not (hence the namespacing rule in the task).

---------------------------------------------------------------------------------------------------------

## 3. Real tables (fact)

`acore_characters` has **163 tables**. Every one is classified below; the classification is the first draft of the
exporter's *table registry* (see risk R5: a test must fail when a new per-character table appears that is in no
class). Columns are shown as they exist in the schema fixture. There are no foreign keys between these tables
(only `highrisk_chest_item -> highrisk_chest` and three `mail_server_*` links), so relationships below are
**logical** and enforced only by the server code.

### A. Portable character state (v1 scope) (22 tables)

| Table | Role | Columns |
|---|---|---|
| `character_account_data` | per-character client blobs: macros (type 5) only in v1 | guid,type,time,data |
| `character_action` | active action bar rows | guid,spec,button,action,type |
| `character_appearance` | CoA wardrobe: equipped appearance per category; v1.1 | guid,category_id,appearance_id |
| `character_appearance_outfit` | CoA wardrobe outfits; v1.1 | guid,name,appearances |
| `character_appearance_settings` | CoA wardrobe visibility flags; v1.1 | guid,can_see_item,can_see_spell |
| `character_equipmentsets` | equipment-manager sets; item0..18 are item GUIDs, `setguid` is a global counter (remap); v1.1 | guid,setguid,setindex,name,iconname,ignore_mask,item0,item1,item2,item3,item4,item5,item6,item7,item8,item9,item10,item11,item12,item13,item14,item15,... |
| `character_gifts` | wrapped-gift content of an item; `item_guid` (remap) | guid,item_guid,entry,flags |
| `character_glyphs` | glyph slots | guid,talentGroup,glyph1,glyph2,glyph3,glyph4,glyph5,glyph6 |
| `character_inventory` | bag/slot placement; `item` is an item GUID (remap) | guid,bag,slot,item |
| `character_pet` | hunter/summon pets; `id` is a pet number (remap) | id,entry,owner,modelid,CreatedBySpell,PetType,level,exp,Reactstate,name,renamed,slot,curhealth,curmana,curhappiness,savetime,abdata |
| `character_pet_declinedname` | pet declined names; `id` is the pet number | id,owner,genitive,dative,accusative,instrumental,prepositional |
| `character_queststatus` | active quests + objective progress | guid,quest,status,explored,timer,mobcount1,mobcount2,mobcount3,mobcount4,itemcount1,itemcount2,itemcount3,itemcount4,itemcount5,itemcount6,playercount |
| `character_queststatus_rewarded` | completed quests | guid,quest,active |
| `character_reputation` | faction standing | guid,faction,standing,flags |
| `character_settings` | CoA gameplay state (`core.*` sources): active spec, build slots, spec bars, resets, starter flag | guid,source,data |
| `character_skills` | skill values | guid,skill,value,max |
| `character_spell` | known spells (CoA talents are spells) | guid,spell,specMask |
| `character_talent` | stock talents (empty on CoA, kept for stock/Wildcard compatibility) | guid,spell,specMask |
| `characters` | identity/progression/appearance columns only (see section 6); position, flags and cache columns are not carried | guid,account,name,race,class,gender,level,xp,money,skin,face,hairStyle,hairColor,facialStyle,bankSlots,restState,playerFlags,position_x,position_y,pos... |
| `item_instance` | item state; `guid` item GUID (remap), `owner_guid` player GUID (remap), `creatorGuid`/`giftCreatorGuid` foreign player GUIDs (zero) | guid,itemEntry,owner_guid,creatorGuid,giftCreatorGuid,count,duration,charges,flags,enchantments,randomPropertyId,durability,playedTime,text |
| `mod_craftsmans_codex` | extra profession slots (extension `mod:craftsmans-codex`) | guid,slots,first_used_at,last_used_at |
| `pet_spell` | pet spells; `guid` is the pet number (remap) | guid,spell,active |

### B. Portable collections (account-wide, union semantics) (6 tables)

| Table | Role | Columns |
|---|---|---|
| `account_appearance_collection` | CoA wardrobe unlocks, account-wide, INSERT IGNORE only (monotonic) | account_id,appearance_id,source_item |
| `account_vanity_collection` | CoA vanity unlocks, account-wide, INSERT IGNORE only (monotonic) | account_id,item_id |
| `coa_wildcard_skill_card` | Wildcard ruleset only: card collection + progress, account-bound | account,card,progress |
| `coa_wildcard_skill_card_account` | Wildcard ruleset only: bonus pack progress | account,bonus_progress |
| `coa_wildcard_skill_card_pending` | Wildcard ruleset only: unopened cards | account,id,card |
| `coa_wildcard_skill_card_purchase` | Wildcard ruleset only: purchase counters (price escalation) | account,type,count |

### C. Account-level / realm-account state (not portable in v1) (10 tables)

| Table | Role | Columns |
|---|---|---|
| `account_ascension_settings` | character-screen sort order | account_id,sort_order |
| `account_data` | client account-wide blobs | accountId,type,time,data |
| `account_instance_times` | instance entry throttle | accountId,instanceId,releaseTime |
| `account_tutorial` | tutorial flags | accountId,tut0,tut1,tut2,tut3,tut4,tut5,tut6,tut7 |
| `coa_account_warchest` | one-time welcome mail marker (per realm account) | account,claimed_at |
| `coa_wildcard_specialization_cache` | one-time claim marker (per realm account) | account,claimed_at |
| `mod_ascension_bank_item` | personal bank / realm bank items (item GUIDs) | owner_kind,owner_id,tab_index,slot,item_guid |
| `mod_ascension_bank_log` | bank event log | log_id,owner_kind,owner_id,tab_index,event_type,player_guid,item_or_money,stack_count,dest_tab,timestamp |
| `mod_ascension_bank_money` | personal bank / realm bank money | owner_kind,owner_id,money |
| `mod_ascension_bank_tab` | personal bank (owner_kind 0) / realm bank (owner_kind 1) tabs | owner_kind,owner_id,tab_index,name,icon,text |

### D. Per-character state deliberately not portable in v1 (59 tables)

| Table | Role | Columns |
|---|---|---|
| `ascension_manastorm_bonus` | Manastorm run state (D6) | guid,mode,pity,caches |
| `ascension_manastorm_cache` | Manastorm caches; `item` is an item GUID (D6) | item,guid |
| `ascension_manastorm_clear` | Manastorm clears (D6) | guid,mode,depth,scene,mail_id,completed_at |
| `ascension_manastorm_loadout` | Manastorm loadout (D6) | guid,slot,spell |
| `ascension_manastorm_xp` | Manastorm xp (D6) | guid,amount |
| `ascension_player_ticket` | GM tickets | id,account,creator_guid,creator,title,category,priority,affected_character,status,assigned_to,closed_by_creator,created,closed,locale |
| `ascension_player_ticket_message` | GM tickets | ticket,id,from_gm,gm_only,sender,message,created,read_by,read_at |
| `character_achievement` | achievements (out of scope for v1) | guid,achievement,date |
| `character_achievement_offline_updates` | queued achievement updates | guid,update_type,arg1,arg2,arg3 |
| `character_achievement_progress` | achievement criteria (out of scope for v1) | guid,criteria,counter,date |
| `character_arena_stats` | arena MMR (explicit non-goal) | guid,slot,matchMakerRating,maxMMR |
| `character_ascension_state` | character-screen active/inactive flag (realm-local) | guid,active |
| `character_aura` | temporary auras (explicit non-goal) | guid,casterGuid,itemGuid,spell,effectMask,recalculateMask,stackCount,amount0,amount1,amount2,base_amount0,base_amount1,base_amount2,maxDuration,remain... |
| `character_banned` | bans (explicit non-goal) | guid,bandate,unbandate,bannedby,banreason,active |
| `character_battleground_random` | random-BG daily flag | guid |
| `character_brew_of_the_month` | event marker | guid,lastEventId |
| `character_coa_lfg_settings` | LFG preference (realm-local) | guid,composition_mode,challenge_size |
| `character_declinedname` | name declensions (ruRU only) | guid,genitive,dative,accusative,instrumental,prepositional |
| `character_entry_point` | battleground/transport return point | guid,joinX,joinY,joinZ,joinO,joinMapId,taxiPath0,taxiPath1,mountSpell |
| `character_homebind` | regenerated at the destination (hearthstone) | guid,mapId,zoneId,posX,posY,posZ |
| `character_instance` | instance locks (explicit non-goal) | guid,instance,permanent,extended |
| `character_queststatus_daily` | realm-clock reset state | guid,quest,time |
| `character_queststatus_monthly` | realm-clock reset state | guid,quest |
| `character_queststatus_seasonal` | realm event state | guid,quest,event |
| `character_queststatus_weekly` | realm-clock reset state | guid,quest |
| `character_social` | friends/ignore (explicit non-goal) | guid,friend,flags,note |
| `character_spell_cooldown` | cooldowns (explicit non-goal) | guid,spell,category,item,time,needSend |
| `character_stats` | derived cache, rewritten on save | guid,maxhealth,maxpower1,maxpower2,maxpower3,maxpower4,maxpower5,maxpower6,maxpower7,strength,agility,stamina,intellect,spirit,armor,resHoly,resFire,r... |
| `character_worldforged_loot` | world pickup ledger (realm-local) | guid,spawn_id,entry,looted_at |
| `coa_bot_gear_history` | bots only | guid,slot,item_guid,entry,replaced_at |
| `coa_bot_gear_queue` | bots only | guid |
| `coa_challenge_completion` | challenge history (D6) | guid,challengeId,level,completeTime,startTime |
| `coa_challenge_failure` | challenge history (D6) | guid,challengeId,level,deaths,failTime |
| `coa_character_challenge` | challenge/hardcore state (ruleset-local, decision D6) | guid,challengeId,level,deaths,hunger,thirst,startTime |
| `coa_character_condition` | challenge flags (D6) | guid,flag |
| `coa_character_fatigue` | fatigue (D6) | guid,challengeId,fatigue |
| `coa_character_gamemode` | game-mode flag (D6) | guid,gameMode |
| `coa_character_gamemode_lives` | game-mode deaths (D6) | guid,gameMode,deaths |
| `coa_character_looted_item` | challenge loot ledger (D6) | guid,itemGuid |
| `coa_character_objective` | challenge objectives (D6) | guid,challengeId,objective |
| `coa_character_survival` | survival needs (D6) | guid,hunger,thirst |
| `coa_custom_trial` | custom trials authored by the character (D6) | guid,trialId,title,about,icon,author |
| `coa_custom_trial_active` | (D6) | guid,trialId,startTime |
| `coa_custom_trial_completion` | (D6) | guid,trialId,startTime,completeTime |
| `coa_custom_trial_entry` | (D6) | guid,trialId,challengeId,level,description |
| `coa_custom_trial_vote` | (D6) | guid,trialId,upvote,downvote |
| `corpse` | corpse state (explicit non-goal) | guid,posX,posY,posZ,orientation,mapId,phaseMask,displayId,itemCache,bytes1,bytes2,guildId,flags,dynFlags,time,corpseType,instanceId |
| `highrisk_chest` | High-Risk death-chest escrow (realm-local) | id,owner,map,phase,x,y,z,o,gold,original_gold,gold_claimant,created,gold_claimed_at |
| `highrisk_chest_item` | High-Risk escrow items, item GUIDs (realm-local) | chest_id,slot,item_guid,entry,count,claimant,claimed_at,active_item_guid |
| `item_loot_storage` | loot container contents (item GUID) | containerGUID,itemid,count,item_index,randomPropertyId,randomSuffix,follow_loot_rules,freeforall,is_blocked,is_counted,is_underthreshold,needs_quest,c... |
| `item_refund_instance` | refund window of an item (item GUID) | item_guid,player_guid,paidMoney,paidExtendedCost |
| `item_soulbound_trade_data` | BoP trade window of an item (item GUID) | itemGuid,allowedPlayers |
| `lfg_data` | LFG state | guid,dungeon,state |
| `mod_coa_bot_guild_gather_orders` | bots only | bot_guid,item_entry,target_count,gathered_count,remaining_count,target_map,target_x,target_y,target_z,has_target_location |
| `pet_aura` | temporary auras | guid,casterGuid,spell,effectMask,recalculateMask,stackCount,amount0,amount1,amount2,base_amount0,base_amount1,base_amount2,maxDuration,remainTime,rema... |
| `pet_spell_cooldown` | cooldowns | guid,spell,category,time |
| `player_anticheat_alert` | anticheat log | id,date,account,guid,name,reason,details,size |
| `quest_tracker` | quest analytics | id,character_guid,quest_accept_time,quest_complete_time,quest_abandon_time,completed_by_gm,core_hash,core_revision |
| `recovery_item` | item-recovery module (realm-local) | Id,Guid,ItemEntry,Count,DeleteDate |

### E. Realm operations (never portable) (66 tables)

`active_arena_season`, `addons`, `arena_team`, `arena_team_member`, `auctionhouse`, `banned_addons`, `battleground_deserters`, `bugreport`, `calendar_events`, `calendar_invites`, `channels`, `channels_bans`, `channels_rights`, `chat_filter`, `coa_keepers_scroll_blessing`, `coa_squid_migrations`, `creature_respawn`, `ethereal_bazaar_meta`, `ethereal_bazaar_stock`, `game_event_condition_save`, `game_event_save`, `gameobject_respawn`, `gm_subsurvey`, `gm_survey`, `gm_ticket`, `group_member`, `groups`, `guild`, `guild_bank_eventlog`, `guild_bank_item`, `guild_bank_right`, `guild_bank_tab`, `guild_eventlog`, `guild_member`, `guild_member_withdraw`, `guild_rank`, `instance`, `instance_reset`, `instance_saved_go_state_data`, `lag_reports`, `log_arena_fights`, `log_arena_memberstats`, `log_encounter`, `log_money`, `mail`, `mail_items`, `mail_server_character`, `mail_server_template`, `mail_server_template_conditions`, `mail_server_template_items`, `petition`, `petition_sign`, `playerbots_arena_team_names`, `playerbots_guild_names`, `playerbots_names`, `pool_quest_save`, `profanity_name`, `pvpstats_battlegrounds`, `pvpstats_players`, `reserved_name`, `spam_reports`, `updates`, `updates_include`, `warden_action`, `world_state`, `worldstates`

### 3.1 Primary keys and logical relationships of the portable set

| Table | Primary key | Logical reference |
|---|---|---|
| `characters` | `guid` | `account` -> `acore_auth.account.id` |
| `character_inventory` | **`item`** (unique) and unique `(guid, bag, slot)` | `guid` -> character, `item` -> `item_instance.guid`, `bag` -> the bag's *item guid* (0 = on the character) |
| `item_instance` | `guid` | `owner_guid` -> character (0 for bank/escrow items), `creatorGuid`/`giftCreatorGuid` -> *other* characters |
| `character_spell` | `(guid, spell)` | `spell` -> DBC |
| `character_skills` | `(guid, skill)` | |
| `character_reputation` | `(guid, faction)` | |
| `character_queststatus` | `(guid, quest)` | `quest` -> `acore_world.quest_template` |
| `character_queststatus_rewarded` | `(guid, quest)` | |
| `character_action` | `(guid, spec, button)` | |
| `character_glyphs` | `(guid, talentGroup)` | |
| `character_pet` | `id` (global pet number) | `owner` -> character |
| `pet_spell` | `(guid, spell)` | `guid` -> `character_pet.id` |
| `character_pet_declinedname` | `id` | `id` -> `character_pet.id`, `owner` |
| `character_settings` | `(guid, source)` | `source` is a free-form string (see section 7) |
| `character_account_data` | `(guid, type)` | |
| `character_equipmentsets` | `setguid` (global 64-bit auto-increment) | `guid` -> character, `item0..18` -> `item_instance.guid` |
| `character_gifts` | item guid | `item_guid` -> `item_instance.guid` |
| `character_appearance` / `_outfit` / `_settings` | `(guid, category_id)` / `(guid, name)` / `guid` | CoA wardrobe, per character |
| `account_appearance_collection` | `(account_id, appearance_id)` | `source_item` = item entry that unlocked it |
| `account_vanity_collection` | `(account_id, item_id)` | |

---------------------------------------------------------------------------------------------------------

## 4. Identifier domains and what must be remapped (fact)

| Domain | Allocated by | Where it appears | Remap on import |
|---|---|---|---|
| **Character guid** (32-bit) | `ObjectMgr` generator, seeded `MAX(guid)+1` at startup (`ObjectMgr.cpp:7633-7636`) | every `guid` / `owner_guid` / `owner` column, `character_inventory.guid`, `mod_ascension_bank_item.owner_id`, ... | **yes**, one new value |
| **Item guid** (32-bit) | item generator, seeded `MAX(item_instance.guid)+1` (`ObjectMgr.cpp:7639`) | `item_instance.guid`, `character_inventory.item` **and** `.bag` (a bag is an item), `character_gifts.item_guid`, `character_equipmentsets.item0..18`, `character_aura.itemGuid`, `item_refund_instance`, `item_soulbound_trade_data`, `item_loot_storage.containerGUID`, `mod_ascension_bank_item.item_guid`, `highrisk_chest_item.item_guid`, `ascension_manastorm_cache.item`, `coa_character_looted_item.itemGuid`, `mail_items.item_guid`, `auctionhouse.itemguid`, `guild_bank_item.item_guid` | **yes**, a map old->new, applied to *both* `item` and `bag` |
| **Pet number** (32-bit) | `ObjectMgr::GeneratePetNumber`, seeded `MAX(character_pet.id)+1` (`ObjectMgr.cpp:8169-8214`) | `character_pet.id`, `pet_spell.guid`, `pet_aura.guid`, `pet_spell_cooldown.guid`, `character_pet_declinedname.id` | **yes** |
| **Equipment set guid** (64-bit) | `_equipmentSetGuid`, seeded `MAX(setguid)+1` (`ObjectMgr.cpp:7669`) | `character_equipmentsets.setguid` | yes (v1.1) |
| **Account id** | auth database | `characters.account`, every `account_*` / `coa_wildcard_*` collection table | **yes**, destination account (provisioned per realm) |
| Mail id, guild id, arena team id, group, instance id | server | mail, guild, ... | not carried |
| Foreign **player** guids inside portable rows | n/a | `item_instance.creatorGuid`, `giftCreatorGuid` (item crafted/gifted by *another* character), `character_aura.casterGuid` (auras are excluded anyway) | set to **0** (or to the new guid when it equals the character itself); the crafter's *name* may be kept as display text |

Observed facts that affect the remap design:

* `character_inventory` is keyed by **`item`**, so one item can be in exactly one place; a remap that changes both
  `item` and `bag` consistently is required (a bag's contents point to the bag's item guid).
* Player guids **are reused**: the generator restarts from `MAX(guid)+1`, so deleting the newest character frees its
  guid. `Player::DeleteFromDB` (`Player.cpp:4217`) deletes only stock tables; module tables are cleaned by
  `OnPlayerDelete`/`OnPlayerDeleteFromDB` hooks, and the audit found only **two** such handlers
  (`AscensionCompat.cpp:6650` for outfits, `AscensionManastorm.cpp:1859`). Rows of `character_appearance`,
  `coa_character_*`, `character_worldforged_loot`, `mod_craftsmans_codex`, ... can therefore be **orphaned** and attach
  to a later character that reuses the guid. The importer must therefore run a complete
  `DELETE ... WHERE guid = <new guid>` over every per-character table (portable or not) before inserting, inside the
  same transaction.
* Item guids of *other* tables (mail, auction, guild bank) are never moved, so remapping stays inside the portable set.

---------------------------------------------------------------------------------------------------------

## 5. Portable vs world-local (fact + proposal)

| | Portable (v1) | World-local (never carried) |
|---|---|---|
| Identity | name, race, class, gender, skin/face/hair/facial style, `playerFlags` subset | `account`, `guid`, creation date, `at_login`, ban/delete info |
| Progression | level, xp, money, `arenaPoints`, honor columns, `knownCurrencies`, titles (`knownTitles`, `chosenTitle`), explored zones, taxi mask | `online`, `totaltime`/`leveltime`, rest state, `logout_time` |
| Build | known spells, skills, glyphs, stock talents, `character_settings` `core.*` sources, `talentGroupsCount`/`activeTalentGroup`, `extraBonusTalentCount`, `resettalents_*` | cooldowns, auras |
| Inventory | all `character_inventory` rows (equipment 0-18, bags 19-22, backpack 23-38, **bank 39-66 and bank bags 67-73**, keyring 86-117, currency-token slots 118-149) with their `item_instance` rows | buyback slots 74-85 (not persisted), mail, auction, guild bank |
| Quests | active + objectives, completed | daily / weekly / monthly / seasonal (tied to the destination's clock), quest tracker |
| Gameplay | action bars (`character_action` + per-spec bars in settings), macros, equipment sets | friends, ignore, guild, group, instance/raid locks, battleground and arena state |
| Position | **nothing**; destination chooses | map, x/y/z, orientation, zone, transport, corpse, `death_expire_time`, homebind |

Positions: `Player::LoadFromDB` falls back to the race/class start location when the map or coordinates are invalid
(`PlayerStorage.cpp:5202`, `:5247`) and recreates `character_homebind` from `playercreateinfo` when the row is missing or
invalid (`PlayerStorage.cpp:7173-7222`). Both fall back to the class/race start **only if `playercreateinfo` has an
entry for that race/class pair** - an import must verify that, because otherwise the character cannot load. The
importer should write the destination's own start position (read from its world database) instead of relying on the
error path.

---------------------------------------------------------------------------------------------------------

## 6. `characters` column map (fact + proposal)

| Columns | Class | Notes |
|---|---|---|
| `guid`, `account` | remapped | destination values |
| `name` | identity | `varchar(25)`, collation **`utf8mb4_bin`** (case-sensitive; widened by `rev_1789996074311168500.sql`). Name uniqueness is enforced by the server through its name cache, not by a unique key |
| `race`, `class`, `gender`, `skin`, `face`, `hairStyle`, `hairColor`, `facialStyle` | identity | CoA class/race numbers; namespaced in the model |
| `level`, `xp` | progression | |
| `money` | progression | `int unsigned`; the server clamps to `MAX_MONEY_AMOUNT` = 2,147,483,646 copper (~214,748 gold) (`Player.h:926`, `PlayerStorage.cpp:5120`). Largest real value seen: 200,060,523 |
| `arenaPoints`, `totalHonorPoints`, `todayHonorPoints`, `yesterdayHonorPoints`, `totalKills`, `todayKills`, `yesterdayKills`, `knownCurrencies` | progression | `knownCurrencies` must be consistent with the currency-token items (see the Rune of Ascension migration comment: the Currency page stays blank if the bit is 0) |
| `chosenTitle`, `knownTitles` | progression | cheap to carry; achievements themselves stay out of v1 |
| `exploredZones`, `taximask`, `bankSlots`, `stable_slots`, `talentGroupsCount`, `activeTalentGroup`, `extraBonusTalentCount`, `resettalents_cost`, `resettalents_time`, `grantableLevels`, `watchedFaction`, `actionBars`, `ammoId`, `innTriggerId` | progression/build | `actionBars` is the "which bars are visible" bitmask; it is not the bars |
| `playerFlags`, `extra_flags` | partly | only the cosmetic flags (helm/cloak visibility); never GM/ghost/AFK bits |
| `position_*`, `map`, `instance_id`, `instance_mode_mask`, `orientation`, `zone`, `trans_*`, `transguid`, `taxi_path`, `death_expire_time` | world-local | rewritten |
| `health`, `power1..7`, `drunk`, `restState`, `rest_bonus`, `is_logout_resting`, `cinematic`, `totaltime`, `leveltime`, `logout_time`, `latency`, `online`, `at_login`, `equipmentCache`, `order`, `creation_date`, `deleteInfos_*`, `deleteDate` | world-local / derived | `equipmentCache` is rebuilt on every save; `at_login` must **not** contain `AT_LOGIN_FIRST` (0x20) or the first-login rewards run again |

---------------------------------------------------------------------------------------------------------

## 7. CoA-specific state (fact)

### 7.1 Talents, builds, spec bars live in `character_spell` + `character_settings`

* `character_settings` has primary key `(guid, source)`; `data` is a space-separated list of unsigned integers
  (`PlayerSettingsStore::SerializeSettingsData`, `PlayerSettings.cpp`). Sources starting with **`core.` are mandatory
  gameplay state** and are loaded even when optional player settings are disabled (`PlayerSettings.cpp:116-120`).
* Sources found in code (`grep` over `src/` and `modules/`):

| Source | Meaning |
|---|---|
| `core.ascension_active_spec` | active specialization id (61 call sites; present on 1857 of 1861 real characters) |
| `core.ascension_starter` / `core.ascension_starter_live` | starter kit already granted - **must travel**, or the starter items are granted again at the destination (duplication) |
| `core.ascension_slot.active`, `core.ascension_slot.<n>` | active build slot and per-slot data (`AscensionCompat.cpp:1970-1977`) |
| `core.ascension_build.<specId>` and `core.ascension_slot.<n>.build.<specId>` | stored talent picks: `[count, entryId*10+rank, ...]` (`AscensionCompat.cpp:1970-2060`) |
| `core.ascension_bar.<specId>` and `core.ascension_slot.<n>.bar.<specId>` | stored action bar per spec: `[count, button, spell, ...]` |
| `core.ascension_reset_credits` | ability/talent reset credits |
| `core.spell_charge.<spellId>` | spell charge state (one row per spell; 40+ distinct spells in real data) |
| `core.wildcard`, `core.wildcard.{cards,spec,path,bars,scrolls,scrolls.spec2,rewards,rerolls,repurchase,unlearned,startercards}` | Wildcard ruleset state |
| `core.destiny_weaver`, `core.dynamic_xp.preset`, `core.runemaster.echoes`, `core.coa_prestige`, `core.coa_prestige_bar`, `core.coa_glory`, `core.ascension.tutorial_*` | module / UI state |
| `coa.bot.gear`, `coa.bot_profile`, `coa.gear_pref_*`, `coa.gameplay_test`, `coa.highrisk` | bots, preferences, tests - not portable |

* **Proposal:** treat `character_settings` as a namespaced key -> integer-vector map (`coa:settings:<source>`),
  exported by an **allowlist of source patterns** that lives in a versioned policy file inside the Manager. Sources not
  on the allowlist are preserved in the canonical snapshot as opaque extension data and are not imported (the
  quarantine behaviour demanded for modded content).
* Because a CoA "talent" is a spell, **stock `character_talent` can be empty and the build still be fully described**
  by `character_spell` + the sources above. The audit found no other store.

### 7.2 Spells

`character_spell` holds the entire spellbook (avg ~1,284, max 1,568 rows for a level-80 character). `specMask`
distinguishes dual-spec ownership. Because spells are a set of ids from a ruleset-global DBC, they can be carried as a
sorted integer set (delta + varint) at a few KB per character.

### 7.3 Pets

`character_pet` stores **every** pet type, not only hunter pets: the real data has 5 rows, all `PetType = 0` (summon
pets of CoA's own classes), 0 hunter pets (CoA has no stock Hunter). `Pet::SavePetToDB` (`Pet.cpp:502`) writes the
current pet and stabled pets with `slot` (current / stable 1-4 / "not in slot"). Portable v1: all `character_pet` rows
for the character + `pet_spell` + `character_pet_declinedname`; **not** `pet_aura` and `pet_spell_cooldown`. The pet
number is remapped (section 4). `characters.stable_slots` must be carried or the stable looks empty.

### 7.4 Items

`item_instance` columns: `itemEntry, count, duration, charges (5 ints), flags, enchantments (36 ints = 12 slots x
[id, duration, charges]), randomPropertyId (negative = suffix), durability, playedTime, text, creatorGuid,
giftCreatorGuid`. Real data: all 46k rows have a 72-character enchantment string; 22 items have a random property.
Item `flags` bits that carry state: soulbound, refundable, BoP-tradeable, wrapped, ...

Special item-related state outside `item_instance`:

| State | Table | v1 |
|---|---|---|
| wrapped gift | `character_gifts` | carry (part of item) |
| refund window (2h) | `item_refund_instance` | drop; the server clears the flag itself after load |
| BoP trade window | `item_soulbound_trade_data` | drop; the server clears the flag |
| bag contents | `character_inventory.bag` | structural |
| personal bank | `mod_ascension_bank_*` `owner_kind = 0` (items stored as `item_instance` rows with **`owner_guid = 0`**) | **decision D3**: a character's personal bank is really inventory; excluded rows would be silently lost if the realm ever deleted the character |
| realm bank | `owner_kind = 1`, keyed by **account** | not portable (account/realm local) |

### 7.5 Account-wide collections

| Collection | Table | Key | Written by | Notes |
|---|---|---|---|---|
| Wardrobe appearances | `account_appearance_collection(account_id, appearance_id, source_item)` | account | `INSERT IGNORE` (`AscensionCompat.cpp:5018`) | monotonic |
| Vanity items | `account_vanity_collection(account_id, item_id)` | account | `INSERT IGNORE` (`:5038`, `:5053`) | monotonic. When config `UNLOCK_ALL_VANITY` is on, vanity is *not tracked at all* (the table stays empty) |
| Equipped appearance / outfits / visibility | `character_appearance`, `character_appearance_outfit`, `character_appearance_settings` | **character** | `REPLACE` / `DELETE` of that character's rows | per character, small |
| Wildcard skill cards | `coa_wildcard_skill_card*` (4 tables) | account | Wildcard module | season-bound, Wildcard ruleset only |

Facts for Phase 6:

* **No code path deletes from the two account collections** (`grep` for `DELETE` on them: none). Union semantics are
  safe and match reality.
* They are **loaded once at login** into `PlayerCollectionState` (`AscensionCompat.cpp:4912-4965`). New unlocks written
  to the database while a character is online are invisible to that session. The Manager must therefore write
  collections only for a character that is not logged in (before join / after the final sync).
* Real size: the fixture's wardrobe has 387 rows for the whole realm. Even a generous estimate for a full collector is a
  few thousand ids; a sorted, delta + varint encoded `u32` set is ~1-2 bytes per id. The 10k / 50k test sizes in the
  task are good stress tests but not realistic production sizes.
* Collections are keyed by the **destination** account id, so a `PortableProfile` -> realm-account mapping is part of
  account provisioning (Phase 12).

### 7.6 Ruleset-local progression that is **not** portable in v1 (decision D6)

Challenge / hardcore state (`coa_character_*`, `coa_challenge_*`, `coa_custom_trial*`), Manastorm run state
(`ascension_manastorm_*`), High-Risk chests, world-pickup ledger. Moving a character with an active challenge or
game-mode flag would silently turn a hardcore character into a normal one. Proposal: **refuse "Make Portable"** (or
warn) for characters that have rows in the challenge/game-mode tables.

---------------------------------------------------------------------------------------------------------

## 8. What the server does when it loads and saves a character (fact)

### 8.1 Load (`PlayerStorage.cpp:5022` `LoadFromDB`, query list `CharacterHandler.cpp:72`)

The login holder runs ~30 queries by guid (spells, quests, inventory, actions, reputation, skills, glyphs, talents,
account data, settings, ...). Validation done at load:

* `characters.account` must equal the logged-in account, else the load fails (`:5050`). **Importing needs the
  destination account id in `characters.account`.**
* Name is re-validated (`CheckPlayerName`); an invalid or reserved name sets `AT_LOGIN_RENAME` and fails the load.
* Race/class pair must have a `playercreateinfo` row.
* Money is clamped; titles/explored zones loaded as raw blobs.
* Banned characters (`character_banned`) cannot load.

### 8.2 Save (`PlayerStorage.cpp:7244` `SaveToDB`)

One transaction per save: character row, entry point, inventory (**only changed items**), quests, talents, spells,
cooldowns, actions, auras, skills, achievements, reputation, equipment sets, glyphs, settings, stats, current pet.
Save triggers: autosave interval, logout, and explicit calls. Everything is `InnoDB`, so one realm save is atomic.

### 8.3 Item load is destructive (`PlayerStorage.cpp:6104` `_LoadItem`, `:5972` `_LoadInventory`)

| Condition at load | What the server does |
|---|---|
| `itemEntry` not in `item_template` | **deletes** the item from `character_inventory` and `item_instance` |
| `Item::LoadFromDB` fails | deletes it |
| limited to another map/zone | deletes it |
| `ITEM_FLAG_CONJURED` and logged out > 15 min | deletes it |
| holiday item outside its event | deletes it |
| cannot be placed/equipped (`CanStoreItem` / `CanEquipItem` fails, e.g. level requirement, missing bag) | **removes it from the inventory table and mails it** to the character |
| refundable without `item_refund_instance` row / BoP-tradeable without trade row | clears the flag |

### 8.4 Consequence: export is a *view*, not the truth (proposal)

Because of 8.3 a realm may legitimately return fewer items than it was given. Therefore:

* Every portable item carries a stable **portable item id** (UUIDv7) in the model. The per-realm mapping
  `portable item id <-> local item guid` is stored in the Manager (extension of `CharacterServerMapping`).
* `Export` at checkpoint/final sync is diffed against the **base snapshot** that was imported at join (`revision_at_join`).
  An item that was in the base, is missing from the export, and is *known to have been filtered* (unknown entry at
  destination, or found in mail at the destination) stays in the canonical snapshot as **quarantined**; an item that is
  missing without such an explanation is treated as deleted by the player (vendored/destroyed).
* This is the same mechanism that Phase 7 (mod extensions) and Phase 8 (cap projection) need, so it should be in the
  data model from Phase 1 even though the diff logic comes later.

---------------------------------------------------------------------------------------------------------

## 9. Transactions and atomicity (fact + proposal)

* All relevant tables are InnoDB: one `START TRANSACTION ... COMMIT` can cover the whole import (character, items,
  pets, settings, spells, quests, ...). On failure, the connection closing (or `ROLLBACK`) removes everything: **no
  half-imported character is possible if the import is a single transaction**.
* The core's `PlayerDumpReader::LoadDump` (`PlayerDump.cpp:764-968`) is the existing reference: it builds one
  `CharacterDatabaseTransaction`, remaps items/pets/mails/equipment sets from the live generators, commits, then
  refreshes the name cache and advances the in-memory counters (`sCharacterCache->AddCharacterCacheEntry`,
  `GetGenerator<HighGuid::Item>().Set(...)`, `_hiPetNumber`, ...). It also sets a temporary name and `AT_LOGIN_RENAME` on
  a name clash and caps accounts at 10 characters.
* Which operations can be transactional **from outside** (Manager -> MySQL): all inserts and the guid
  allocation *if the server is not running*. With the server running they can be transactional in MySQL but are not
  coherent with the server's memory (section 10).
* Restoring a database requires the game servers to be stopped; the Manager already enforces this (`backup.rs` header),
  which is a useful precedent for "destructive replacement of a character requires the character, or the whole server,
  to be offline".

---------------------------------------------------------------------------------------------------------

## 10. Running realm vs. external writes (fact -> decision D1)

Facts:

1. Id counters are in memory, seeded once from `MAX(...)` at startup (`ObjectMgr.cpp:7633-7681`, `:8169`).
2. `CharacterCache` (name -> guid -> account/race/class/level) is filled once at startup
   (`CharacterCache.cpp:63`). `RefreshCacheEntry` exists and is reachable from the console as `.cache refresh`
   (`cs_cache.cpp:38`, `Console::Yes`); whether that command can address a guid the cache does not know yet (it takes a
   `PlayerIdentifier`) must be verified before relying on it.
3. `World::UpdateRealmCharCount` refreshes `acore_auth.realmcharacters`; a direct insert leaves the character count of
   the account wrong.
4. Collections are cached per login; `Player` state is only written back by the Player object (an online character's
   next save overwrites anything written to its rows underneath it).
5. `Player::SaveToDB` is not called every 60 s (autosave interval, default 15 minutes in stock AzerothCore; the real
   value is in the realm's `worldserver.conf`; the core default is `PlayerSaveInterval = 900000` ms, `WorldConfig.cpp:223`).
   `.save` (`cs_misc.cpp:122`) is `Console::No`; `.saveall` (`cs_misc.cpp:123`) is `Console::Yes` but calls
   `SaveToDB` for **every** online player, bots included, in one pass (`ObjectAccessor.cpp:286-293`), which is not a
   per-character checkpoint and gets expensive on a realm with many bots.
6. The Manager's RA client exposes **a fixed set of typed operations** only (`ra.rs:1-2`, `:68-166`); adding an
   operation is a deliberate code change.

Options:

| | A. Offline-only import | B. Reserved id range + external writes | C. In-core importer |
|---|---|---|---|
| How | server stopped (or realm DB offline) when importing; Manager allocates `MAX+1..` itself | Manager allocates ids from a high reserved range (e.g. >= 2^31) so it can never collide with the live counters; name cache refreshed by restart or a new command | new core console command(s) that take a Manager-produced portable payload and do remap/insert/commit/cache-refresh inside the server (extend the `.pdump` machinery) |
| Core change | none | optional small command to refresh cache | new commands + the CoA tables in the dump list |
| Works for a *public host with players online* | **no** | partly (character invisible to name lookups until refresh; realm char count stale) | **yes** |
| Works for the Phase 3 vertical slice on two disposable DBs | **yes** | yes | yes |
| Risk | lowest | id space gets sparse; subtle cache gaps | largest, but the *safest at runtime* |

**Recommendation:** do Phase 3 and Phase 4 with **A** (it is enough to prove the model and is fully testable on
disposable databases), and decide **C** for production hosting in parallel as a core-side workstream. B is a stopgap and
is not recommended. For C and for the 60-second checkpoints the core needs (all small, console-only, RA-callable):

1. `.character save <name>` - persist one online character now (checkpoint without `.saveall`).
2. `.portable import ...` / `.portable export ...` - or CoA-aware extension of `.pdump` - doing remap + commit + cache
   refresh in the server.
3. If B is ever used: confirm that the existing `.cache refresh` can address a not-yet-cached guid, otherwise add the
   guid variant plus a `UpdateRealmCharCount` call.

Whatever option is chosen, **an import must refuse a character that is online**, check `characters.online`, and verify
`acore_auth.account.online` for the target account.

---------------------------------------------------------------------------------------------------------

## 11. Account and authentication side (fact)

* `acore_auth.account` (`salt`, `verifier`, ...) is created through the world console (`ra.rs:80 create_account`, the
  typed RA operations), because the SRP6 verifier depends on the username. The Manager also contains `srp6.rs`.
* Bot accounts (`COABOTHOST*`) and the Manager's own `COAMANAGER` are filtered out of account listings
  (`accounts.rs:22-28`); the export must apply the same filter so that bots are never offered for "Make Portable".
* `acore_auth.realmcharacters(realmid, acctid, numchars)` must be kept correct after an import
  (`World::UpdateRealmCharCount` at `World.cpp:1660`).
* The Manager can already discover a realm's databases, ports and credentials for both bundled-MySQL and Docker
  installations (`db.rs:68-120`), including the Wildcard schema routing.

---------------------------------------------------------------------------------------------------------

## 12. Level cap and content scaling (fact, input for Phase 8)

* The realm's level cap is `CONFIG_MAX_PLAYER_LEVEL` (`MaxPlayerLevel`); content scaling computes a progression layout
  from it once at startup and requires a restart when it changes
  (`docs/coa-content-scaling-architecture.md` section 3.1).
* No clamp of a loaded character's level to the cap was found in `LoadFromDB`; the cap only stops XP gain/level-up
  (`Player.cpp:2519`, `:2735`, `:6097`, `:10723`; `PlayerQuest.cpp:808`). A level-80 character on a cap-60 realm would
  currently load *as level 80*.
* Per-viewer scaling (`mod-destiny-weaver`, `LocalLevelScaling`) already scales the **world around** a character and
  never lowers the character (`docs/coa/level-scaling.md`). So a projection that *lowers the character* is new
  behaviour, and it interacts with 8.3: gear above the projected level would be mailed away. Phase 8 therefore needs
  an item-equip/level-requirement override for projected sessions, not just a snapshot transformation.

---------------------------------------------------------------------------------------------------------

## 13. Exact integration points

### 13.1 Manager (`origin/master` `cd3a963`)

| Concern | File | Notes |
|---|---|---|
| Manager-wide state directory | `src-tauri/src/lib.rs:74` `data_dir()` = `%LOCALAPPDATA%\CoAServerManager`; `:947` `remote_dir()` | new `portable\` folder goes here; **not** inside an install's `.manager` folder, because a portable character must exist without any install |
| Atomic file helpers | `crates/coa-core/src/fsx.rs:83,145,151` (`safe_join`, `atomic_write_json`, `read_json`) | reuse for non-DB files |
| Database access | `crates/coa-core/src/db.rs` (`Db::from_repack`, `Account::{Admin,App}`, `query`, `run_sql_file`, Docker via `docker exec`) | CLI only; see risk R4 |
| Realm schemas / modes | `crates/coa-core/src/realms.rs:15-40`, `db.rs:79-92` (`schema_of`), `Db::realm_schema` | CoA vs Wildcard routing |
| Account operations | `crates/coa-core/src/accounts.rs`, `ra.rs` (`create_account`, `set_account_password`, ...), `srp6.rs` | Phase 12 |
| World console | `crates/coa-core/src/ra.rs` | typed ops only; new ops (`save character`, ...) are added here |
| Process state | `process.rs`, `driver.rs`, `layout.rs` | "is the realm stopped?" for offline import |
| Recovery points | `backup.rs` (`create`, `restore_database`, `with_database`) | backup-before-destructive-replacement for Phase 4 reuses this; it already refuses to restore while servers run |
| Schema contract | `schema_check.rs`, `Scripts/database-schema.json`, `character-save-columns.json` | the compatibility fingerprint ("destination characters schema matches contract") |
| Migrations ledger | `migrations.rs` (`coa_manager_migrations` in `acore_world`) | the Manager changes realm databases only through this; **the portable store is a separate local database and must not add tables to realm databases** |
| Networking (Phase 13/14) | `net.rs` (public IP, CGNAT detection), `upnp.rs`, `firewall.rs`, `friends.rs`, `realmlist.rs` | `friends.rs` already advertises the realm address and builds a friend package |
| Player Mode shell | `src/App.tsx` (`view === "remote"`), `src/screens/RemoteClient.tsx`, `crates/coa-core/src/remote_client.rs`, `src-tauri/src/lib.rs:947-1003` | a "no server" mode already exists (host + client path) and is the natural home for My Characters |
| Frontend API layer | `src/lib/api.ts`, Tauri `invoke_handler` at `lib.rs:1523` (83 commands) | add one `portable_*` command group, not one command per UI action |
| i18n | `src/i18n/locales/*.ts`, `tools/check-i18n.mjs` (CI step) | every new string needs a key in en + 4 drafts |
| CI | `.github/workflows/check.yml`: Windows + Ubuntu, `npm ci`, `check-i18n`, `npm run build` (`tsc`), `cargo test --workspace --locked` | any new crate must build on **both** OSes |
| Crates available | `uuid` (feature `v4` only - needs `v7`), `sha2`, `zstd`, `serde_json`, `chrono`, `tempfile`; **no** SQLite, MySQL driver, KDF or bitmap crate | |

### 13.2 Core (`641251c4f`)

| Concern | Location |
|---|---|
| Per-character save | `Player::SaveToDB` `PlayerStorage.cpp:7244`; `_SaveCharacter` `Player.cpp:15547`; `_SaveInventory` `PlayerStorage.cpp:7445`; pet `Pet.cpp:502` |
| Per-character load | `Player::LoadFromDB` `PlayerStorage.cpp:5022`; query holder `CharacterHandler.cpp:72`; `_LoadInventory` `:5972`; `_LoadItem` `:6104`; `Item::LoadFromDB` `Item.cpp:420`; `_LoadHomeBind` `PlayerStorage.cpp:7173`; `_LoadPetStable` `Player.cpp:16741` |
| Character deletion | `Player::DeleteFromDB` `Player.cpp:4217`; module hooks `OnPlayerDelete` (`PlayerScript.cpp:287`) and `OnPlayerDeleteFromDB` |
| Id allocation | `ObjectMgr::SetHighestGuids` `ObjectMgr.cpp:7633`; pet number `:8169`, `:8213`; generator `ObjectGuid.h:284-317` |
| Name cache | `CharacterCache.cpp:63` (load), `:88` (`RefreshCacheEntry`) |
| Existing live-safe import/export | `PlayerDump.cpp:764` (`LoadDump`), table list `:85-118`; commands `cs_character.cpp:43-47` (`pdump load/write/copy`, all `Console::Yes`) |
| CoA settings store | `PlayerSettings.cpp:110-185`; keys in `AscensionCompat.cpp:333-335, 1970-2040` |
| CoA collections | `AscensionCompat.cpp:4912` (load), `:5018`, `:5038` (writes) |
| Commands to extend | `cs_misc.cpp:122-124` (`save`, `saveall`, `kick`), `cs_character.cpp` |

---------------------------------------------------------------------------------------------------------

## 14. Risks

| # | Risk | Severity | Where it bites | Mitigation |
|---|---|---|---|---|
| R1 | External writes into a running realm collide with in-memory counters / stale name cache | High | Phase 3-5 | Option A for Phases 3-4; decision D1 for production; import refuses online characters |
| R2 | Server load-time deletes/mails items; "replace canonical with export" loses them | High | Phase 4, 5, 8 | item identity ids + base-snapshot diff + quarantine (8.4) designed in from Phase 1 |
| R3 | Database is stale between saves; 60 s checkpoint impossible without a per-character save | High | Phase 5 | core command `.character save` (D1/D13) |
| R4 | Untrusted snapshot strings reach SQL through the CLI client | High | Phase 3 import, 15 | prepared statements via a driver crate (D9); validate sizes/charset first; never build SQL from snapshot text |
| R5 | Per-character state outside stock tables is missed (silent half-export) | High | Phase 2 | the classification above becomes a **registry test**: the test loads the schema contract and fails if a table with a `guid`-like column is in no class; fixture tests export a CoA character with a build and compare |
| R6 | Orphan rows + guid reuse attach old data to a new character | Medium | Phase 3, 4 | delete across **all** per-character tables for the target guid inside the import transaction |
| R7 | Collections keyed by destination account id and cached at login | Medium | Phase 6, 12 | write only while the character is offline; profile -> account mapping per realm |
| R8 | Destination lacks items/spells/quests the character has | Medium | Phase 3, 11 | pre-flight: `item_template`/`quest_template` entries via the destination world DB, schema-contract check, namespaced ids; quarantine rather than import |
| R9 | CoA vs Wildcard (and Docker installs, secondary world) take different schemas/ports | Medium | all | the portable layer takes a `RealmTarget {ruleset, schema names, Db}` from existing `realms.rs`/`db.rs`; never hardcodes `acore_characters` |
| R10 | Name clash / reserved name / `utf8mb4_bin` case sensitivity | Low | Phase 3 | follow `PlayerDump`: temporary name + `AT_LOGIN_RENAME`; compare case-insensitively |
| R11 | One-time rewards re-granted (`AT_LOGIN_FIRST`, starter kit, warchest) | Medium | Phase 3 | `at_login = 0`, carry `core.ascension_starter`; account-level claim tables are per realm and not carried |
| R12 | Core version drift between realms (3 revisions in use) | Medium | Phase 11 | compatibility keyed by schema contract + `format_version`, shown before JOIN |
| R13 | `money`/currency overflow, absurd payloads | Low | Phase 15 | caps from the server code (money <= 2,147,483,646), size limits per field and per snapshot |

---------------------------------------------------------------------------------------------------------

## 15. Decisions needed

| # | Decision | Recommendation | Needed before |
|---|---|---|---|
| **D1** | How a *running public host* receives a character | Phase 3-4 offline (A); plan the in-core command set (C) as a core workstream | Phase 3 starts for production use; not for the disposable-DB slice |
| **D2** | `character_settings` scope | allowlist of `core.*` patterns (versioned policy), the rest opaque/quarantined | Phase 2 |
| **D3** | Personal bank (`mod_ascension_bank_*`, `owner_kind 0`) | include as extension `mod:ascension-bank` in v1.1; realm bank excluded | Phase 2 |
| **D4** | Equipment sets and per-character wardrobe (`character_appearance*`) | v1.1, after the core slice works | Phase 2 |
| **D5** | CoA <-> Wildcard transfer | refuse (different rulesets) | Phase 1 (field in model) |
| **D6** | Challenge / hardcore / Manastorm state | not portable in v1; refuse or warn on "Make Portable" | Phase 2 |
| **D7** | Achievements, titles | titles columns in v1, achievements out | Phase 2 |
| **D8** | Which core branch/commit is the supported target; may the owner accept small core patches in the fork | confirm; Phase 5 and production hosting depend on it | Phase 3/5 |
| **D9** | Database driver in the Manager | add a pure-Rust `mysql` driver crate used only by the portable layer; keep the CLI for existing code | Phase 2 (exporter reads) / 3 |
| **D10** | Local store | SQLite (`rusqlite`, bundled), WAL, `user_version` migrations | Phase 1 |
| **D11** | Name clash policy | temporary name + rename-at-login (as `.pdump`) | Phase 3 |
| **D12** | Daily/weekly/monthly quests | excluded | Phase 2 |
| **D13** | Checkpoint mechanism | `.character save <name>` core command | Phase 5 |

---------------------------------------------------------------------------------------------------------

## Appendix A. Phase 1 - exact proposed plan (not started)

**Goal:** local canonical storage only. No VPS, no relay, no public list, no database access to any realm, no UI.

### A.1 Branch and commits

`feat/portable-characters` (exists, based on `origin/master` `cd3a963`). One commit per step below.

### A.2 New dependencies (crates/coa-core)

| Crate | Why | Check |
|---|---|---|
| `uuid` feature `v7` | UUIDv7 generation (the `v4` feature stays) | pure Rust |
| `rusqlite` with `bundled` | transactions, migrations, blobs, metadata queries | needs a C compiler on the build machine: present on the owner's machine (MSVC) and on both CI runners |

No other new crates in Phase 1. Collections use a hand-written sorted-set + varint encoding (RoaringBitmap is a Phase 6
decision). Snapshot compression reuses `zstd`; hashing reuses `sha2`.

### A.3 Files

```
crates/coa-core/Cargo.toml                     + uuid v7, rusqlite
crates/coa-core/src/lib.rs                     + pub mod portable;
crates/coa-core/src/portable/mod.rs            re-exports, error type
crates/coa-core/src/portable/versions.rs       PORTABLE_CHARACTER_FORMAT_VERSION = 1, PORTABLE_COLLECTION_FORMAT_VERSION = 1,
                                               EXTENSION_FORMAT_VERSION = 1, REGISTRY_PROTOCOL_VERSION = 1, RELAY_PROTOCOL_VERSION = 1
crates/coa-core/src/portable/ids.rs            CharacterId / ProfileId / SnapshotId / PortableItemId (UUIDv7 newtypes)
crates/coa-core/src/portable/model.rs          PortableCharacter v1 (section 5/6/7), Extension container (opaque), CollectionSet
crates/coa-core/src/portable/snapshot.rs       envelope (character_id, revision, snapshot_format_version, created_at, source_server_id,
                                               content_hash), canonical JSON + zstd, size limits, hash
crates/coa-core/src/portable/collection.rs     sorted u32 set (delta+varint), union, collection_revision/hash
crates/coa-core/src/portable/store.rs          SQLite store, transactions, migrations, history/pruning
crates/coa-core/src/portable/migrations/001_init.sql
docs/PORTABLE_FORMAT.md                        the on-disk/serialised format and its version rules
```

No Tauri command, no frontend change, no changelog fragment (internal; `AGENTS.md` asks for fragments only for
player/admin-visible changes).

### A.4 Storage

`%LOCALAPPDATA%\CoAServerManager\portable\portable.db`, created lazily on first use. Opened with WAL, `foreign_keys=ON`,
`PRAGMA user_version` for migrations (forward-only, each in one transaction, refuses a database written by a *newer*
format). Unit tests use temp directories.

```
profile(profile_id TEXT PK, created_at, format_version)
character(character_id TEXT PK, profile_id FK, ruleset TEXT, name, race TEXT, class TEXT, gender INT,
          revision INT NOT NULL, created_at, updated_at, archived INT)
snapshot(character_id FK, revision INT, snapshot_format_version INT, created_at, source_server_id TEXT,
         content_hash BLOB, size INT, payload BLOB, note TEXT, PRIMARY KEY(character_id, revision))
character_server_mapping(character_id FK, server_id TEXT, local_guid INT, last_revision INT, state TEXT,
         PRIMARY KEY(character_id, server_id), UNIQUE(server_id, local_guid))
item_mapping(character_id, server_id, portable_item_id TEXT, local_item_guid INT, PRIMARY KEY(character_id, server_id, portable_item_id))
collection(profile_id FK, kind TEXT, collection_revision INT, collection_hash BLOB, format_version INT, payload BLOB,
           PRIMARY KEY(profile_id, kind))
setting(key TEXT PK, value TEXT)                       -- history_keep (default 20), ...
```

### A.5 Operations (Rust API, synchronous, transactional)

* `create_character(profile, model, source_server_id)` -> `CharacterId` (UUIDv7), revision 1, in one transaction.
* `commit_snapshot(character_id, expected_revision, model, source_server_id)` -> new revision; fails with
  `StaleRevision{expected, current}` when `expected != current`; `BEGIN IMMEDIATE`, so concurrent writers serialise.
* `rollback_to(character_id, revision)` -> creates **a new revision N+1** whose payload is the old one (revision
  numbers never decrease; freshness stays revision-based).
* `prune(character_id)` keeps the newest `history_keep` revisions, **never** the current one.
* `get_snapshot`, `list_revisions`, `bind_server(character_id, server_id, local_guid)`, `merge_collection_union(...)`.
* Hard limits enforced at deserialisation (prepares Phase 15): max payload bytes, max items, max spells,
  max extension blobs; decompression capped by declared size.

### A.6 Tests (all in `crates/coa-core`, no network, no realm DB)

| Test | Proves |
|---|---|
| `create_character_identity` | UUIDv7 (version bits, time-ordered), revision 1, row counts |
| `revision_increments_monotonically` | 1,2,3,... without gaps |
| `stale_revision_is_rejected` | commit with an old `expected_revision` -> `StaleRevision`, nothing written |
| `duplicate_uuid_is_rejected` | inserting an existing `character_id` fails and rolls back |
| `rollback_creates_new_revision` | state equal to the old one, revision strictly greater |
| `prune_keeps_current` | history cap respected, current never removed |
| `serialization_roundtrip` | model -> snapshot -> model structurally equal (incl. empty and maximal characters) |
| `unknown_extension_is_preserved` | opaque extension survives a roundtrip byte-for-byte |
| `snapshot_hash_is_stable` | same model -> same `content_hash`; one changed field -> different |
| `oversized_payload_is_refused` | limits |
| `migration_idempotent_and_refuses_newer_db` | `user_version` handling |
| `collection_union_never_removes` | union semantics, hash unchanged when nothing new |
| `collection_set_roundtrip_10k_50k` | compact encoding, sorted, small |
| `mapping_unique_constraints` | `(server_id, local_guid)` cannot be bound to two characters |

### A.7 Gate

* `cargo test --workspace --locked` (baseline today: 242 passing; all new tests added on top, none removed)
* `npm ci && node tools/check-i18n.mjs && npm run build` (CI parity; no frontend change expected, run to prove it)
* `cargo build` on Linux is covered by CI; locally the Windows build must pass.

### A.8 Migration impact

None for existing installs: no change to any realm database, to `install.json`, to recovery points or to the update
flow. A new directory is created on first use of the portable layer only. Removing the feature leaves one unused
folder.

### A.9 What Phase 1 deliberately does not decide

How realm rows are exported/imported (Phase 2/3), the item-diff algorithm (Phase 4), checkpoint transport (Phase 5), the
final collection encoding (Phase 6), transfer-PIN KDF (Phase 9/15). The model leaves room for all of them
(`item_mapping`, extension container, collection `format_version`).
