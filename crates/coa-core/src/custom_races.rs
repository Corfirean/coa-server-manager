//! Management of client-side patches and assets for Custom Races.
//!
//! Handles detecting, enabling/disabling (via `.disabled` extension renaming),
//! and downloading/extracting client patches (Esteria and additional races)
//! into the player's WoW client directory.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::download::Cancel;
use crate::error::{Error, Result};
use crate::fsx;

pub const CLIENT_VERSION: &str = "2.0.3";
pub const MANIFEST_FILENAME: &str = "custom_races_patch_manifest.txt";
/// Files earlier releases installed that the current release no longer ships (removed on update).
#[cfg(test)]
const RETIRED_FILES: &[&str] = &["Ascension.exe"];
/// The client archives of the coa-custom-2.0.3 release. Clients 1 to 11 and 17 are byte for byte the archives of the
/// earlier releases and keep the file names the Manager already saved them under, so a verified copy in the cache or in
/// Downloads is not fetched again. The server archive is read only for its client addons; the SkinCollection
/// (Skinshifter) addon in Client 17 is not installed (see RACE_ADDONS). Order matters: a later archive replaces a file of
/// an earlier one; Client 17 carries the newer Ascension_Subclass and Client 18 comes last.
pub const ARCHIVES: &[(&str, &str)] = &[
    ("CoA-Custom-V2-Client-1.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-1.zip"),
    ("CoA-Custom-V2-Client-2.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-2.zip"),
    ("CoA-Custom-V2-Client-3.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-3.zip"),
    ("CoA-Custom-V2-Client-4.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-4.zip"),
    ("CoA-Custom-V2-Client-5.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-5.zip"),
    ("CoA-Custom-V2-Client-6.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-6.zip"),
    ("CoA-Custom-V2-Client-7.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-7.zip"),
    ("CoA-Custom-V2-Client-8.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-8.zip"),
    ("CoA-Custom-V2-Client-9.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-9.zip"),
    ("CoA-Custom-V2.0.2-Client-10.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-10.zip"),
    ("CoA-Custom-V2.0.2-Client-11.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-11.zip"),
    ("CoA-Custom-V2.0.3-Client-12.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-12.zip"),
    ("CoA-Custom-V2.0.3-Client-13.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-13.zip"),
    ("CoA-Custom-V2.0.3-Client-14.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-14.zip"),
    ("CoA-Custom-V2.0.3-Client-15.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-15.zip"),
    ("CoA-Custom-V2.0.3-Client-16.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-16.zip"),
    ("CoA-Custom-V2.0.3-Server.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Server.zip"),
    ("CoA-Custom-V2.0.2-Client-17.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-17.zip"),
    ("CoA-Custom-V2.0.3-Client-18.zip", "https://github.com/ilusixn/CustomRacesCoa/releases/download/coa-custom-2.0.3/CoA-Custom-V2.0.3-Client-18.zip"),
];

pub const RACE_MPQS: &[&str] = &[
    "patch-T.MPQ",
    "patch-ZE1.MPQ",
    "patch-ZE2.MPQ",
    "patch-ZE3.MPQ",
    "patch-ZE5.MPQ",
    "patch-ZE6.MPQ",
    "patch-ZE7.MPQ",
    "patch-ZE8.MPQ",
    "patch-ZE9.MPQ",
    "patch-ZEA.MPQ",
    "patch-ZES.MPQ",
    "patch-ZEZV.MPQ",
    "patch-ZEZW.MPQ",
    "patch-ZEZX.MPQ",
    "patch-ZEZY.MPQ",
    "patch-ZEZZ.MPQ",
    "patch-ZEZZA.MPQ",
    "patch-ZEZZB.MPQ",
    "patch-ZEZZC.MPQ",
    "patch-ZEZZD.MPQ",
    "patch-ZEZZE.MPQ",
    "patch-ZEZZF.MPQ",
    "patch-ZEZZG.MPQ",
    "patch-ZEZZH.MPQ",
    "patch-ZEZZI.MPQ",
    "patch-ZG.MPQ",
    "patch-ZH.MPQ",
    "patch-ZHA.MPQ",
    "patch-ZHM.MPQ",
    "patch-ZJ.MPQ",
    "patch-ZK.MPQ",
    "patch-ZM.MPQ",
    "patch-ZVU.MPQ",
    "patch-ZZE.MPQ",
    "patch-ZZR.MPQ",
    "patch-ZZS.MPQ",
    "patch-ZZT.MPQ",
    "patch-ZZU.MPQ",
    "patch-ZZV.MPQ",
    "patch-ZZW.MPQ",
    "patch-ZZX.MPQ",
    "patch-ZZZ.MPQ",
    "patch-ZZZA.MPQ",
    "patch-ZZZC.MPQ",
    "patch-ZZZD.MPQ",
    "patch-ZZZE.MPQ",
    "patch-ZZZF.MPQ",
    "patch-ZZZG.MPQ",
    "patch-ZZZZ.MPQ",
    "patch-ZZZZA.MPQ",
    "patch-ZZZZB.MPQ",
    "patch-ZZZZC.MPQ",
    "patch-ZZZZD.MPQ",
    "patch-ZZZZE.MPQ",
    "patch-ZZZZS.MPQ",
    "patch-ZZZZT.MPQ",
    "patch-ZZZZW.MPQ",
    "patch-ZZZZX.MPQ",
    "patch-ZZZZY.MPQ",
    "patch-ZZZZZ.MPQ",
    "patch-ZZZZZA.MPQ",
    "patch-ZZZZZB.MPQ",
    "patch-ZZZZZC.MPQ",
    "patch-ZZZZZM.MPQ",
    "patch-ZZZZZN.MPQ",
    "patch-ZZZZZO.MPQ",
    "patch-ZZZZZP.MPQ",
    "patch-ZZZZZQ.MPQ",
    "patch-ZZZZZR.MPQ",
    "patch-ZZZZZS.MPQ",
    "patch-ZZZZZT.MPQ",
    "patch-ZZZZZU.MPQ",
    "patch-ZZZZZV.MPQ",
    "patch-ZZZZZW.MPQ",
    "patch-ZZZZZX.MPQ",
    "patch-ZZZZZY.MPQ",
    "patch-ZZZZZY-SUBMA.MPQ",
    "patch-ZZZZZY-SUBVA.MPQ",
    "patch-ZZZZZY-SUBVB.MPQ",
    "patch-ZZZZZY-SUBVC.MPQ",
    "patch-ZZZZZY-SUBXA-HD.MPQ",
    "patch-ZZZZZY-SUBXB-HD.MPQ",
    "patch-ZZZZZY-SUBXC-HD.MPQ",
    "patch-ZZZZZZ.MPQ",
    "patch-ZZZZZZA.MPQ",
    "patch-ZZZZZZB.MPQ",
    "patch-ZZZZZZC.MPQ",
    "patch-ZZZZZZD.MPQ",
    "patch-ZZZZZZE.MPQ",
    "patch-ZZZZZZF.MPQ",
    "patch-retail-001.mpq",
    "patch-retail-002.mpq",
    "patch-retail-003.mpq",
    "patch-retail-004.mpq",
    "patch-retail-005.mpq",
];

pub const RACE_ADDONS: &[&str] = &[
    "Ascension_Subclass/Ascension_Subclass.lua",
    "Ascension_Subclass/Ascension_Subclass.toc",
    "Ascension_Subclass/Bindings.xml",
    "Ascension_Subclass/SubclassBar.lua",
    "Ascension_Subclass/SubclassBonk.lua",
    "Ascension_Subclass/SubclassCarry.lua",
    "Ascension_Subclass/SubclassCastBar.lua",
    "Ascension_Subclass/SubclassChoices.lua",
    "Ascension_Subclass/SubclassGlow.lua",
    "Ascension_Subclass/SubclassNodes.lua",
    "Ascension_Subclass/SubclassOrbs.lua",
    "Ascension_Subclass/SubclassPairs.lua",
    "Ascension_Subclass/SubclassPetBar.lua",
    "Ascension_Subclass/SubclassPowerBar.lua",
    "Ascension_Subclass/SubclassRaceBackground.lua",
    "Ascension_Subclass/Textures/IceFrame.tga",
    "CoAHideArmor/CoAHideArmor.lua",
    "CoAHideArmor/CoAHideArmor.toc",
    "CoAQuestSafe/CoAQuestSafe.lua",
    "CoAQuestSafe/CoAQuestSafe.toc",
    "CoAWC2Cursor/CoAWC2Cursor.lua",
    "CoAWC2Cursor/CoAWC2Cursor.toc",
    "CoA_DevMenu/CoA_DevMenu.lua",
    "CoA_DevMenu/CoA_DevMenu.lua.before_v2",
    "CoA_DevMenu/CoA_DevMenu.lua.before_v3",
    "CoA_DevMenu/CoA_DevMenu.toc",
    "CoA_DevMenu/CoA_DevMenu_CoA.lua",
    "CoA_DevMenu/CoA_DevMenu_Combat.lua",
    "CoA_DevMenu/CoA_DevMenu_Create.lua",
    "CoA_DevMenu/CoA_DevMenu_Create.lua.before_v4",
    "CoA_DevMenu/CoA_DevMenu_Create.lua.before_v5",
    "CoA_DevMenu/CoA_DevMenu_Ext.lua",
    "CoA_DevMenu/CoA_DevMenu_Ext.lua.before_v3",
    "CoA_DevMenu/CoA_DevMenu_Ext.lua.before_v4",
    "CoA_DevMenu/CoA_DevMenu_Help.lua",
    "CoA_DevMenu/CoA_DevMenu_Items.lua",
    "CoA_DevMenu/CoA_DevMenu_Me.lua",
    "CoA_DevMenu/CoA_DevMenu_Server.lua",
    "CoA_DevMenu/CoA_DevMenu_Share.lua",
    "CoA_DevMenu/EmoteData.lua",
    "CoA_TalentReset/CoA_TalentReset.lua",
    "CoA_TalentReset/CoA_TalentReset.toc",
    "CoA_Teleports/CoA_Teleports.lua",
    "CoA_Teleports/CoA_Teleports.toc",
    "CoA_Teleports/TeleportData.lua",
    "SpellActivationOverlay-for-CoA/LICENSE",
    "SpellActivationOverlay-for-CoA/SpellActivationOverlay-for-CoA.toc",
    "SpellActivationOverlay-for-CoA/SpellActivationOverlay.lua",
    "SpellActivationOverlay-for-CoA/SpellActivationOverlay.xml",
    "SpellActivationOverlay-for-CoA/changelog.md",
    "SpellActivationOverlay-for-CoA/classes/barbarian.lua",
    "SpellActivationOverlay-for-CoA/classes/bloodmage.lua",
    "SpellActivationOverlay-for-CoA/classes/chronomancer.lua",
    "SpellActivationOverlay-for-CoA/classes/common.lua",
    "SpellActivationOverlay-for-CoA/classes/cultist.lua",
    "SpellActivationOverlay-for-CoA/classes/deathknight.lua",
    "SpellActivationOverlay-for-CoA/classes/druid.lua",
    "SpellActivationOverlay-for-CoA/classes/felsworn.lua",
    "SpellActivationOverlay-for-CoA/classes/guardian.lua",
    "SpellActivationOverlay-for-CoA/classes/hunter.lua",
    "SpellActivationOverlay-for-CoA/classes/knightofxoroth.lua",
    "SpellActivationOverlay-for-CoA/classes/mage.lua",
    "SpellActivationOverlay-for-CoA/classes/monk.lua",
    "SpellActivationOverlay-for-CoA/classes/necromancer.lua",
    "SpellActivationOverlay-for-CoA/classes/paladin.lua",
    "SpellActivationOverlay-for-CoA/classes/priest.lua",
    "SpellActivationOverlay-for-CoA/classes/primalist.lua",
    "SpellActivationOverlay-for-CoA/classes/pyromancer.lua",
    "SpellActivationOverlay-for-CoA/classes/ranger.lua",
    "SpellActivationOverlay-for-CoA/classes/reaper.lua",
    "SpellActivationOverlay-for-CoA/classes/rogue.lua",
    "SpellActivationOverlay-for-CoA/classes/runemaster.lua",
    "SpellActivationOverlay-for-CoA/classes/shaman.lua",
    "SpellActivationOverlay-for-CoA/classes/starcaller.lua",
    "SpellActivationOverlay-for-CoA/classes/stormbringer.lua",
    "SpellActivationOverlay-for-CoA/classes/suncleric.lua",
    "SpellActivationOverlay-for-CoA/classes/templar.lua",
    "SpellActivationOverlay-for-CoA/classes/tinker.lua",
    "SpellActivationOverlay-for-CoA/classes/venomancer.lua",
    "SpellActivationOverlay-for-CoA/classes/warlock.lua",
    "SpellActivationOverlay-for-CoA/classes/warrior.lua",
    "SpellActivationOverlay-for-CoA/classes/witchdoctor.lua",
    "SpellActivationOverlay-for-CoA/classes/witchhunter.lua",
    "SpellActivationOverlay-for-CoA/components/aura.lua",
    "SpellActivationOverlay-for-CoA/components/bucket.lua",
    "SpellActivationOverlay-for-CoA/components/display.lua",
    "SpellActivationOverlay-for-CoA/components/effect.lua",
    "SpellActivationOverlay-for-CoA/components/events.lua",
    "SpellActivationOverlay-for-CoA/components/external.lua",
    "SpellActivationOverlay-for-CoA/components/glow.lua",
    "SpellActivationOverlay-for-CoA/components/hash.lua",
    "SpellActivationOverlay-for-CoA/components/item.lua",
    "SpellActivationOverlay-for-CoA/components/overlay.lua",
    "SpellActivationOverlay-for-CoA/components/project.lua",
    "SpellActivationOverlay-for-CoA/components/rune.lua",
    "SpellActivationOverlay-for-CoA/components/shutdown.lua",
    "SpellActivationOverlay-for-CoA/components/sound.lua",
    "SpellActivationOverlay-for-CoA/components/spell.lua",
    "SpellActivationOverlay-for-CoA/components/targetspells.lua",
    "SpellActivationOverlay-for-CoA/components/targetwatch.lua",
    "SpellActivationOverlay-for-CoA/components/tr.lua",
    "SpellActivationOverlay-for-CoA/components/trigger.lua",
    "SpellActivationOverlay-for-CoA/components/util.lua",
    "SpellActivationOverlay-for-CoA/components/variable.lua",
    "SpellActivationOverlay-for-CoA/libs/LibActionButton-1.0/LibActionButton-1.0.lua",
    "SpellActivationOverlay-for-CoA/libs/LibActionButton-1.0/LibActionButton-1.0.toc",
    "SpellActivationOverlay-for-CoA/libs/LibButtonGlow-1.0/LibButtonGlow-1.0.lua",
    "SpellActivationOverlay-for-CoA/libs/LibButtonGlow-1.0/LibButtonGlow-1.0.toc",
    "SpellActivationOverlay-for-CoA/libs/LibStub/LibStub.lua",
    "SpellActivationOverlay-for-CoA/libs/LibStub/LibStub.toc",
    "SpellActivationOverlay-for-CoA/options/InterfaceOptionsPanels.lua",
    "SpellActivationOverlay-for-CoA/options/InterfaceOptionsPanels.xml",
    "SpellActivationOverlay-for-CoA/options/classoptions.lua",
    "SpellActivationOverlay-for-CoA/options/db.lua",
    "SpellActivationOverlay-for-CoA/options/defaults.lua",
    "SpellActivationOverlay-for-CoA/options/glowoptions.lua",
    "SpellActivationOverlay-for-CoA/options/overlayoptions.lua",
    "SpellActivationOverlay-for-CoA/options/questions.lua",
    "SpellActivationOverlay-for-CoA/options/variables.lua",
    "SpellActivationOverlay-for-CoA/options/variants.lua",
    "SpellActivationOverlay-for-CoA/textures/bandits_guile.blp",
    "SpellActivationOverlay-for-CoA/textures/eclipse_moon.blp",
    "SpellActivationOverlay-for-CoA/textures/eclipse_sun.blp",
    "SpellActivationOverlay-for-CoA/textures/fury_of_stormrage_yellow.blp",
    "SpellActivationOverlay-for-CoA/textures/high_tide.blp",
    "SpellActivationOverlay-for-CoA/textures/icon64.blp",
    "SpellActivationOverlay-for-CoA/textures/maelstrom_weapon_1.blp",
    "SpellActivationOverlay-for-CoA/textures/maelstrom_weapon_2.blp",
    "SpellActivationOverlay-for-CoA/textures/maelstrom_weapon_3.blp",
    "SpellActivationOverlay-for-CoA/textures/maelstrom_weapon_4.blp",
    "SpellActivationOverlay-for-CoA/textures/mask.blp",
    "SpellActivationOverlay-for-CoA/textures/maskzero.blp",
    "SpellActivationOverlay-for-CoA/textures/rkm128.blp",
    "SpellActivationOverlay-for-CoA/textures/texname.lua",
    "SpellActivationOverlay-for-CoA/textures/thrill_of_the_hunt_1.blp",
    "SpellActivationOverlay-for-CoA/textures/thrill_of_the_hunt_2.blp",
    "SpellActivationOverlay-for-CoA/textures/thrill_of_the_hunt_3.blp",
    "SpellActivationOverlay-for-CoA/variables/actionusable.lua",
    "SpellActivationOverlay-for-CoA/variables/aurastacks.lua",
    "SpellActivationOverlay-for-CoA/variables/custom.lua",
    "SpellActivationOverlay-for-CoA/variables/execute.lua",
    "SpellActivationOverlay-for-CoA/variables/holypower.lua",
    "SpellActivationOverlay-for-CoA/variables/itemset.lua",
    "SpellActivationOverlay-for-CoA/variables/nativesao.lua",
    "SpellActivationOverlay-for-CoA/variables/stance.lua",
    "SpellActivationOverlay-for-CoA/variables/talent.lua",
    "UnBot/OnlineFrame.xml",
];

pub const EXCLUDED_BOT_RACES_RECOMMENDED: &str =
    "9,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128";

pub const EXCLUDED_BOT_RACES_EXPERIMENTAL: &str = "65,72,77,80,81";

/// Finds all active playerbots.conf files on this server (root, coa-bots folder, and secondary realm).
pub fn playerbots_conf_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let candidates = [
        root.join("Core/configs/modules/playerbots.conf"),
        root.join("coa-bots/Core/configs/modules/playerbots.conf"),
        root.join("coa-bots/core/configs/modules/playerbots.conf"),
        crate::multiworld::root(root).join("Core/configs/modules/playerbots.conf"),
    ];
    for path in candidates {
        if path.is_file() {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        for dist in [
            root.join("Core/configs/modules/playerbots.conf.dist"),
            root.join("coa-bots/Core/configs/modules/playerbots.conf.dist"),
        ] {
            if dist.is_file() {
                let target = dist.with_extension("");
                if fs::copy(&dist, &target).is_ok() {
                    paths.push(target);
                    break;
                }
            }
        }
    }
    paths
}

/// Checks whether playerbots are configured to allow custom races.
/// Returns true if AiPlayerbot.ExcludedBotRaces exists and does not exclude general custom races.
pub fn is_bot_custom_races_allowed(root: &Path) -> Option<bool> {
    if let Ok(bytes) = fs::read(root.join("Core/configs/modules/coa_custom_races.conf")) {
        if let Ok(conf) = crate::config::parser::ConfFile::parse_bytes(&bytes) {
            if let Some(value) = conf.get("CoACustomRaces.BotsEnable") {
                return Some(matches!(value.trim().trim_matches('"'), "1" | "true"));
            }
        }
    }
    for path in playerbots_conf_paths(root) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(conf) = crate::config::parser::ConfFile::parse_bytes(&bytes) {
                if let Some(val) = conf.get("AiPlayerbot.ExcludedBotRaces") {
                    let clean = val.trim().trim_matches(['"', '\'']);
                    let excluded: std::collections::BTreeSet<u32> = clean
                        .split(',')
                        .filter_map(|race| race.trim().parse().ok())
                        .collect();
                    return Some(
                        (1..=128)
                            .filter(|race| ![1, 2, 3, 4, 5, 6, 7, 8, 10, 11].contains(race))
                            .any(|race| !excluded.contains(&race)),
                    );
                }
            }
        }
    }
    None
}

/// Sets AiPlayerbot.ExcludedBotRaces in all discovered playerbots.conf files.
pub fn sync_bot_races_config(root: &Path, allow_custom_races: bool) -> Result<()> {
    let excluded = if allow_custom_races {
        EXCLUDED_BOT_RACES_EXPERIMENTAL
    } else {
        EXCLUDED_BOT_RACES_RECOMMENDED
    };
    for path in playerbots_conf_paths(root) {
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(mut conf) = crate::config::parser::ConfFile::parse_bytes(&bytes) {
                if let Some(value) = conf.get("AiPlayerbot.ExcludedBotRaces") {
                    let clean = value.trim().trim_matches(['"', '\'']);
                    let owned = String::from_utf8_lossy(&bytes)
                        .contains("CoA Custom: races new random bots never get")
                        && matches!(
                            clean,
                            EXCLUDED_BOT_RACES_RECOMMENDED | EXCLUDED_BOT_RACES_EXPERIMENTAL
                        );
                    if !owned {
                        continue;
                    }
                }
                conf.set(
                    "AiPlayerbot.ExcludedBotRaces",
                    &format!("\"{excluded}\""),
                    &[
                        "CoA Custom: races new random bots never get",
                        "Off (recommended): excludes all added races",
                        "On (experimental): allows stable added races",
                    ],
                );
                fsx::atomic_write(&path, conf.to_text().as_bytes())?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientPatchState {
    NotInstalled,
    Enabled,
    Disabled,
    PartiallyInstalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientPatchStatus {
    pub state: ClientPatchState,
    pub version: Option<String>,
    pub installed_mpqs: usize,
    pub total_mpqs: usize,
    pub has_root_files: bool,
    pub client_path: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PatchFile {
    path: String,
    sha256: String,
}

fn patch_files(client: &Path) -> Result<Vec<PatchFile>> {
    let manifest = fs::read_to_string(client.join(MANIFEST_FILENAME)).unwrap_or_default();
    let hashes = if manifest.lines().any(|line| line == "# CoA Custom 1.5") {
        include_str!("custom_races_hashes_1_5.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.6") {
        include_str!("custom_races_hashes_1_6.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.7") {
        include_str!("custom_races_hashes_1_7.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.8") {
        include_str!("custom_races_hashes_1_8.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.8.3") {
        include_str!("custom_races_hashes_1_8_3.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.9.1") {
        include_str!("custom_races_hashes_1_9_1.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 1.9.2") {
        include_str!("custom_races_hashes_1_9_2.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 2.0") {
        include_str!("custom_races_hashes_2_0.json")
    } else if manifest.lines().any(|line| line == "# CoA Custom 2.0.2") {
        include_str!("custom_races_hashes_2_0_2.json")
    } else {
        include_str!("custom_races_hashes.json")
    };
    let bundled: std::collections::BTreeMap<String, String> = serde_json::from_str(hashes)?;
    let mut files = std::collections::BTreeMap::new();
    for line in manifest.lines() {
        let path = if let Some(name) = line.strip_prefix("MPQ:") {
            if name.contains(['/', '\\']) {
                return Err(Error::Invalid(
                    "Invalid custom races patch manifest.".into(),
                ));
            }
            Some(format!("Data/{name}"))
        } else if let Some(name) = line.strip_prefix("ROOT:") {
            Some(name.to_string())
        } else {
            line.strip_prefix("ADDON:")
                .map(|name| format!("Interface/AddOns/{name}"))
        };
        if let Some(path) = path {
            fsx::safe_join(client, &path)?;
            files.insert(
                path.clone(),
                bundled.get(&path).cloned().unwrap_or_default(),
            );
        }
    }
    if files.is_empty() {
        files = bundled;
    }
    for line in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("SHA256:"))
    {
        let (hash, path) = line
            .split_once(':')
            .ok_or_else(|| Error::Invalid("Invalid custom races checksum.".into()))?;
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !files.contains_key(path)
        {
            return Err(Error::Invalid("Invalid custom races checksum.".into()));
        }
        files.insert(path.into(), hash.into());
    }
    Ok(files
        .into_iter()
        .map(|(path, sha256)| PatchFile { path, sha256 })
        .collect())
}

fn suffix(path: &Path, extension: &str) -> PathBuf {
    path.with_file_name(format!(
        "{}.{extension}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// SHA-256 of a payload file, remembered for this run of the Manager. V2 is 8.7 GB, and every status check and
/// switch verifies all of it; a rename keeps size, creation and write time, while a replaced file changes them.
fn cached_sha256(path: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::UNIX_EPOCH;
    static HASHES: OnceLock<Mutex<HashMap<(String, u64, u128, u128), String>>> = OnceLock::new();
    let meta = fs::metadata(path).ok()?;
    let nanos = |time: io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
    };
    // The name without `.disabled`: switching renames only that suffix.
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let name = name.strip_suffix(".disabled").unwrap_or(&name).to_string();
    let key = match (nanos(meta.modified()), nanos(meta.created())) {
        (Some(modified), Some(created)) => Some((name, meta.len(), modified, created)),
        _ => None,
    };
    let hashes = HASHES.get_or_init(Mutex::default);
    if let Some(hash) = key.as_ref().and_then(|key| hashes.lock().ok()?.get(key).cloned()) {
        return Some(hash);
    }
    let hash = fsx::sha256_file(path).ok()?;
    if let (Some(key), Ok(mut hashes)) = (key, hashes.lock()) {
        hashes.insert(key, hash.clone());
    }
    Some(hash)
}

fn matches_patch(path: &Path, file: &PatchFile) -> bool {
    path.is_file() && cached_sha256(path).is_some_and(|hash| hash == file.sha256)
}

/// Whether disabling the patch must hide this file. The stock client loads only the MPQs in Data,
/// dinput8.dll and the addons; the rest (Data/Retail, VulkanShaders, the Esteria data files) is read by the
/// custom races executable and its libraries alone, so it stays in place: thousands of renames bought nothing.
/// A file that replaced one of the player's own (an `.original` exists) is always switched.
fn is_switched(client: &Path, file: &PatchFile) -> bool {
    let lower = file.path.to_ascii_lowercase();
    let entry_point = lower.starts_with("interface/addons/")
        || lower
            .strip_prefix("data/")
            .is_some_and(|name| !name.contains('/') && name.ends_with(".mpq"))
        || (!lower.contains('/')
            && [".dll", ".exe", ".bat"]
                .iter()
                .any(|extension| lower.ends_with(extension)));
    entry_point || suffix(&client.join(&file.path), "original").is_file()
}

/// Characters of custom races (RaceID above 11) on the active realm: a game client without the client patch
/// crashes when one of them comes into view, so the server keeps them out of the world while custom races are
/// off. `None` when the database is not running.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomRaceCharacters {
    pub players: u32,
    pub companion_bots: u32,
    pub squid_bots: u32,
}

pub fn custom_race_characters(root: &Path) -> Result<Option<CustomRaceCharacters>> {
    let squid = crate::accounts::squid_bot_prefix(root);
    let characters = crate::realms::state(root)
        .map(|s| s.active)
        .unwrap_or_default()
        .schema("characters")?;
    crate::backup::with_running_database(root, |db| {
        let out = db.query(&format!(
            "SELECT \
             IFNULL(SUM(a.username NOT LIKE 'COABOTHOST%' AND a.username NOT LIKE '{squid}%'), 0), \
             IFNULL(SUM(a.username LIKE 'COABOTHOST%'), 0), \
             IFNULL(SUM(a.username LIKE '{squid}%'), 0) \
             FROM {characters}.characters c JOIN acore_auth.account a ON a.id = c.account WHERE c.race > 11;"
        ))?;
        let numbers: Vec<u32> = out
            .lines()
            .last()
            .unwrap_or("")
            .split('\t')
            .map(|n| n.trim().parse().unwrap_or(0))
            .collect();
        Ok(CustomRaceCharacters {
            players: numbers.first().copied().unwrap_or(0),
            companion_bots: numbers.get(1).copied().unwrap_or(0),
            squid_bots: numbers.get(2).copied().unwrap_or(0),
        })
    })
}

/// Only complete, verified payloads are reported as enabled or disabled.
pub fn client_patch_status(client_path: &Path) -> ClientPatchStatus {
    let data = client_path.join("Data");
    let installed_mpqs = RACE_MPQS
        .iter()
        .filter(|name| data.join(name).is_file() || suffix(&data.join(name), "disabled").is_file())
        .count();
    let has_root_files = client_path.join("EsteriaAppearance.dll").is_file()
        || client_path.join("dinput8.dll").is_file()
        || client_path.join("dinput8.dll.disabled").is_file();
    let manifest = fs::read_to_string(client_path.join(MANIFEST_FILENAME)).unwrap_or_default();
    let current = manifest
        .lines()
        .any(|line| line == format!("# CoA Custom {CLIENT_VERSION}"));
    let state = if !manifest.is_empty()
        && current
        && !client_path.join(".coa-races-transaction").exists()
    {
        match patch_files(client_path) {
            Ok(files)
                if files.iter().all(|f| {
                    matches_patch(&client_path.join(&f.path), f)
                        && !suffix(&client_path.join(&f.path), "disabled").exists()
                }) =>
            {
                ClientPatchState::Enabled
            }
            Ok(files)
                if files.iter().all(|f| {
                    let active = client_path.join(&f.path);
                    matches_patch(&suffix(&active, "disabled"), f)
                        || (!is_switched(client_path, f) && matches_patch(&active, f))
                }) =>
            {
                ClientPatchState::Disabled
            }
            _ => ClientPatchState::PartiallyInstalled,
        }
    } else if !manifest.is_empty()
        || patch_files(client_path).is_ok_and(|files| {
            files
                .iter()
                .filter(|file| file.path.starts_with("Data/") && file.path != "Data/patch-T.MPQ")
                .any(|file| {
                    matches_patch(&client_path.join(&file.path), file)
                        || matches_patch(&suffix(&client_path.join(&file.path), "disabled"), file)
                })
        })
    {
        ClientPatchState::PartiallyInstalled
    } else {
        ClientPatchState::NotInstalled
    };
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("# CoA Custom ").map(str::to_string));
    ClientPatchStatus {
        state,
        version,
        installed_mpqs,
        total_mpqs: RACE_MPQS.len(),
        has_root_files,
        client_path: Some(client_path.to_string_lossy().into_owned()),
    }
}

#[derive(Serialize, Deserialize)]
struct SavedFile {
    path: String,
    existed: bool,
    #[serde(default)]
    moved_to: Option<String>,
}

/// One line of the append-only journal. A payload has thousands of files, so the journal is never rewritten:
/// each step adds one synced line, and recovery replays the lines in order.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalRecord {
    Saved { path: String, existed: bool },
    Moved { index: usize, to: String },
    Returned { index: usize },
}

const LEGACY_JOURNAL: &str = "journal.json";
const JOURNAL: &str = "journal.jsonl";

struct PatchTransaction<'a> {
    root: &'a Path,
    entries: Vec<SavedFile>,
    index: std::collections::HashMap<String, usize>,
    journal: Option<File>,
    finished: bool,
}

// A shared AddOns directory can resolve onto another volume. Publish a complete,
// synced copy beside the destination before deleting the source, so the journal
// always retains recoverable bytes even if copying or publishing fails.
fn move_patch_file(source: &Path, target: &Path) -> io::Result<()> {
    match fs::rename(source, target) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
        Err(error) => return Err(error),
    }
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("Patch target has no parent"))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    fs::copy(source, temporary.path())?;
    temporary.as_file().sync_all()?;
    temporary.persist(target).map_err(|error| error.error)?;
    fs::remove_file(source)
}

fn append_record(journal: &mut File, record: &JournalRecord) -> Result<()> {
    use std::io::Write;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    journal.write_all(&line)?;
    journal.sync_data()?;
    Ok(())
}

/// Replays a journal. A line cut short by a crash is the last one and was never followed by its file
/// operation (each line is synced before the operation it describes), so reading stops there.
fn read_journal(directory: &Path) -> Result<Vec<SavedFile>> {
    let legacy = directory.join(LEGACY_JOURNAL);
    if legacy.is_file() {
        return fsx::read_json(&legacy);
    }
    let bytes = match fs::read(directory.join(JOURNAL)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries: Vec<SavedFile> = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(record) = serde_json::from_slice::<JournalRecord>(line) else {
            break;
        };
        match record {
            JournalRecord::Saved { path, existed } => entries.push(SavedFile {
                path,
                existed,
                moved_to: None,
            }),
            JournalRecord::Moved { index, to } => {
                let entry = entries
                    .get_mut(index)
                    .ok_or_else(|| Error::Invalid("Invalid custom races journal.".into()))?;
                entry.moved_to = Some(to);
            }
            JournalRecord::Returned { index } => {
                let entry = entries
                    .get_mut(index)
                    .ok_or_else(|| Error::Invalid("Invalid custom races journal.".into()))?;
                entry.moved_to = None;
            }
        }
    }
    Ok(entries)
}

impl<'a> PatchTransaction<'a> {
    fn new(root: &'a Path) -> Result<Self> {
        recover_patch_transaction(root)?;
        let directory = root.join(".coa-races-transaction");
        fs::create_dir(&directory)?;
        let journal = fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(directory.join(JOURNAL))?;
        Ok(Self {
            root,
            entries: Vec::new(),
            index: std::collections::HashMap::new(),
            journal: Some(journal),
            finished: false,
        })
    }
    fn relative(&self, path: &Path) -> Result<String> {
        let relative = path
            .strip_prefix(self.root)
            .map_err(|_| Error::Invalid("Patch path escaped the client directory.".into()))?
            .to_string_lossy()
            .replace('\\', "/");
        fsx::safe_join(self.root, &relative)?;
        Ok(relative)
    }
    fn save(&mut self, path: &Path) -> Result<()> {
        let relative = self.relative(path)?;
        if self.index.contains_key(&relative) {
            return Ok(());
        }
        let index = self.entries.len();
        let existed = path.is_file();
        append_record(
            self.journal.as_mut().expect("open journal"),
            &JournalRecord::Saved {
                path: relative.clone(),
                existed,
            },
        )?;
        self.index.insert(relative.clone(), index);
        self.entries.push(SavedFile {
            path: relative,
            existed,
            moved_to: None,
        });
        if existed {
            move_patch_file(
                path,
                &self
                    .root
                    .join(".coa-races-transaction")
                    .join(index.to_string()),
            )?;
        }
        Ok(())
    }
    fn copy(&mut self, source: &Path, target: &Path) -> Result<()> {
        self.save(target)?;
        fs::create_dir_all(
            target
                .parent()
                .ok_or_else(|| Error::Invalid("Patch file has no parent directory.".into()))?,
        )?;
        fs::copy(source, target)?;
        Ok(())
    }
    fn move_file(&mut self, source: &Path, target: &Path) -> Result<()> {
        // Save the destination first so recovery restores the moved source before it.
        self.save(target)?;
        self.save(source)?;
        let index = self.index[&self.relative(source)?];
        let relative = self.relative(target)?;
        append_record(
            self.journal.as_mut().expect("open journal"),
            &JournalRecord::Moved {
                index,
                to: relative.clone(),
            },
        )?;
        self.entries[index].moved_to = Some(relative);
        fs::create_dir_all(target.parent().unwrap())?;
        move_patch_file(
            &self
                .root
                .join(".coa-races-transaction")
                .join(index.to_string()),
            target,
        )?;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        fsx::atomic_write(
            &self.root.join(".coa-races-transaction/committed"),
            b"committed",
        )?;
        self.finished = true;
        // Windows cannot remove a directory while a file in it is open.
        self.journal = None;
        fs::remove_dir_all(self.root.join(".coa-races-transaction"))?;
        Ok(())
    }
}
impl Drop for PatchTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.journal = None;
            let _ = recover_patch_transaction(self.root);
        }
    }
}

fn recover_patch_transaction(root: &Path) -> Result<()> {
    let directory = root.join(".coa-races-transaction");
    if !directory.is_dir() {
        return Ok(());
    }
    if !directory.join("committed").exists() {
        let legacy = directory.join(LEGACY_JOURNAL).is_file();
        let mut entries = read_journal(&directory)?;
        let mut journal = if legacy {
            None
        } else {
            Some(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(directory.join(JOURNAL))?,
            )
        };
        for index in (0..entries.len()).rev() {
            let entry = &entries[index];
            let path = fsx::safe_join(root, &entry.path)?;
            let saved = directory.join(index.to_string());
            if let Some(target) = &entry.moved_to {
                let target = fsx::safe_join(root, target)?;
                if !saved.is_file() && target.is_file() {
                    move_patch_file(&target, &saved)?;
                }
                // Persist recovery progress before restoring the source, making retries safe.
                entries[index].moved_to = None;
                match journal.as_mut() {
                    Some(journal) => append_record(journal, &JournalRecord::Returned { index })?,
                    None => fsx::atomic_write_json(&directory.join(LEGACY_JOURNAL), &entries)?,
                }
            }
            let entry = &entries[index];
            if saved.is_file() {
                if path.is_file() {
                    fs::remove_file(&path)?;
                }
                move_patch_file(&saved, &path)?;
            } else if !entry.existed && path.is_file() {
                fs::remove_file(path)?;
            }
        }
        drop(journal);
    }
    fs::remove_dir_all(directory)?;
    Ok(())
}

fn patch_lock(client: &Path) -> Result<File> {
    use fs4::fs_std::FileExt;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(client.join(".coa-races.lock"))?;
    if !lock.try_lock_exclusive()? {
        return Err(Error::Invalid(
            "Another custom races operation is using this client.".into(),
        ));
    }
    Ok(lock)
}

fn preserve_original(tx: &mut PatchTransaction<'_>, active: &Path) -> Result<()> {
    let original = suffix(active, "original");
    if original.is_file() && fsx::sha256_file(active)? == fsx::sha256_file(&original)? {
        return Ok(());
    }
    if original.is_file() {
        let previous = suffix(
            active,
            &format!("original.previous-{}", uuid::Uuid::new_v4().simple()),
        );
        tx.copy(&original, &previous)?;
    }
    tx.copy(active, &original)
}

/// A file the previous release installed and the new one no longer ships (1.9.2's Ascension.exe, which V2
/// replaced with its own CoACustomRace.exe) is removed if it is still ours, and the player's original comes back.
fn retire_dropped_files(
    tx: &mut PatchTransaction<'_>,
    client: &Path,
    previous: &[PatchFile],
    payload: &std::collections::BTreeSet<String>,
) -> Result<()> {
    for old in previous.iter().filter(|old| !payload.contains(&old.path)) {
        let active = fsx::safe_join(client, &old.path)?;
        if matches_patch(&active, old) {
            tx.save(&active)?;
            let original = suffix(&active, "original");
            if original.is_file() {
                tx.copy(&original, &active)?;
            }
        }
        let disabled = suffix(&active, "disabled");
        if matches_patch(&disabled, old) {
            tx.save(&disabled)?;
        }
    }
    Ok(())
}

/// Preserve launcher replacements, and switch every file owned by the package.
pub fn set_client_patch_enabled(client: &Path, enabled: bool) -> Result<()> {
    if !client.is_dir() {
        return Err(Error::Invalid("Game client directory not found.".into()));
    }
    if crate::client::is_running(client) {
        return Err(Error::Invalid(
            "Close the game before changing custom races patches.".into(),
        ));
    }
    let _lock = patch_lock(client)?;
    recover_patch_transaction(client)?;
    if !client.join(MANIFEST_FILENAME).is_file() {
        return Ok(());
    }
    let files = patch_files(client)?;
    if enabled {
        for file in &files {
            let active = client.join(&file.path);
            if !matches_patch(&active, file) && !matches_patch(&suffix(&active, "disabled"), file) {
                return Err(Error::Invalid(format!("Custom races file {} was replaced or is missing. Repair the client patch before enabling it.", file.path)));
            }
        }
    }
    let mut tx = PatchTransaction::new(client)?;
    for file in files {
        let active = client.join(&file.path);
        let disabled = suffix(&active, "disabled");
        let original = suffix(&active, "original");
        // Data the stock client never reads stays active, and an older Manager's disabled copy comes back.
        if enabled || !is_switched(client, &file) {
            if active.is_file() && !matches_patch(&active, &file) {
                preserve_original(&mut tx, &active)?;
            }
            if matches_patch(&disabled, &file) {
                tx.move_file(&disabled, &active)?;
            }
        } else if matches_patch(&active, &file) {
            if disabled.is_file() && !matches_patch(&disabled, &file) {
                tx.move_file(
                    &disabled,
                    &suffix(
                        &disabled,
                        &format!("replaced-{}", uuid::Uuid::new_v4().simple()),
                    ),
                )?;
            }
            tx.move_file(&active, &disabled)?;
            if original.is_file() {
                tx.copy(&original, &active)?;
            }
        } else if active.is_file() {
            preserve_original(&mut tx, &active)?;
        }
    }
    tx.finish()
}

pub fn uninstall_client_patches(client: &Path) -> Result<()> {
    if crate::client::is_running(client) {
        return Err(Error::Invalid(
            "Close the game before removing custom races patches.".into(),
        ));
    }
    let _lock = patch_lock(client)?;
    recover_patch_transaction(client)?;
    if !client.join(MANIFEST_FILENAME).is_file() {
        return Ok(());
    }
    let mut tx = PatchTransaction::new(client)?;
    for file in patch_files(client)? {
        let active = client.join(&file.path);
        if matches_patch(&active, &file) {
            tx.save(&active)?;
            let original = suffix(&active, "original");
            if original.is_file() {
                tx.copy(&original, &active)?;
            }
        }
        let disabled = suffix(&active, "disabled");
        if active.is_file()
            && !matches_patch(&active, &file)
            && !suffix(&active, "original").is_file()
        {
            tx.move_file(
                &active,
                &suffix(
                    &active,
                    &format!("replaced-{}", uuid::Uuid::new_v4().simple()),
                ),
            )?;
        }
        if disabled.is_file() {
            if !matches_patch(&disabled, &file) {
                tx.move_file(
                    &disabled,
                    &suffix(
                        &disabled,
                        &format!("replaced-{}", uuid::Uuid::new_v4().simple()),
                    ),
                )?;
            } else {
                tx.save(&disabled)?;
            }
        }
    }
    tx.save(&client.join(MANIFEST_FILENAME))?;
    tx.finish()
}

fn stage_root_asset(staging: &Path, relative: &str, reader: &mut impl Read) -> Result<String> {
    let dest = fsx::safe_join(staging, relative)?;
    fs::create_dir_all(
        dest.parent()
            .ok_or_else(|| Error::Invalid("Invalid client asset path.".into()))?,
    )?;
    let mut output = File::create(&dest)?;
    io::copy(reader, &mut output)?;
    let disabled = PathBuf::from(format!("{}.disabled", dest.display()));
    if disabled.is_file() {
        fs::remove_file(disabled)?;
    }
    Ok(relative.to_string())
}

fn archive_job(filename: &str, url: &str, cache: &Path) -> Result<crate::download::Job> {
    let (size, sha256) = match filename {
        "CoA-Custom-V2-Client-1.zip" => (
            1_513_021_586,
            "a9dd51ed2d90fb048d549602f40f5429d56e2df0df5606a38781b358fa369568",
        ),
        "CoA-Custom-V2-Client-2.zip" => (
            1_198_283_005,
            "ab084a55509ccbcc2977a06dd0c01fa879973a1892baaacaeb3ff52f3dfc750e",
        ),
        "CoA-Custom-V2-Client-3.zip" => (
            1_671_130_705,
            "0af34199f0739ce086adb0371d3de7c1a0f5ca9c62ecde6b0efe9286e33874a8",
        ),
        "CoA-Custom-V2-Client-4.zip" => (
            502_385_128,
            "e0bd87172af989fa50a8e2b08ba4321c72b6265047482f051a1db3b80d55bcaa",
        ),
        "CoA-Custom-V2-Client-5.zip" => (
            168_079_711,
            "73364ad1b58c6342641a848a3b20a835472834f600416606be1470386774ca78",
        ),
        "CoA-Custom-V2-Client-6.zip" => (
            1_881_526_537,
            "91db79b7b451856fa9e45daea52e0fe1c04676653dd8a88e6627f04ee7b47759",
        ),
        "CoA-Custom-V2-Client-7.zip" => (
            734_725_114,
            "4c5532d775d206a3b913a6111040bb96b95c683b74c08a6131ed9f38db8e2b39",
        ),
        "CoA-Custom-V2-Client-8.zip" => (
            734_216_670,
            "bdaa1d047e8767bd5715f1e8465d116220597d9c135a43e2dda02e12ce2e5526",
        ),
        "CoA-Custom-V2-Client-9.zip" => (
            195_235_400,
            "0764893543084a12d15578fe734f66ba4519740794a6b3b3a38a08594c38c640",
        ),
        "CoA-Custom-V2.0.2-Client-10.zip" => (
            73_099_250,
            "035720776b66a18b965bbd7262154ea398f5425b8cd116fdfc5f663cd800316e",
        ),
        "CoA-Custom-V2.0.2-Client-11.zip" => (
            178_667_276,
            "9a66f782be47f66ce4ec9f4f70cb1970218fe48950a031b39aa79c091008901b",
        ),
        "CoA-Custom-V2.0.3-Client-12.zip" => (
            1_053_879_059,
            "25d5711963dbae2fa9d030ee883dae6846bd8bcb099ed4cdc27f30fa31561a81",
        ),
        "CoA-Custom-V2.0.3-Client-13.zip" => (
            950_359_687,
            "34efe8dfc1a7d7168298c24c530af5d3625e8a2c48b7183f5c82ad019ece3cf2",
        ),
        "CoA-Custom-V2.0.3-Client-14.zip" => (
            970_584_608,
            "cb1c9b8e2b1fba065bd0f9a252eb20a2f1031828eddef90e3f72e8729124f18c",
        ),
        "CoA-Custom-V2.0.3-Client-15.zip" => (
            1_106_629_165,
            "718d45ef055aa09a48f14f74e2cb14ca3bd782d75a6e1e3f5148146f418af7b6",
        ),
        "CoA-Custom-V2.0.3-Client-16.zip" => (
            14_217_214,
            "cdd9c345d9a0fa8ab9290348f83de6a5bcf05ae543bd3c58aa0d507c7725cb15",
        ),
        "CoA-Custom-V2.0.2-Client-17.zip" => (
            947_478,
            "707e09c8b95926e2d83bdc16adab763901951566caf6bf4fcebfdf4d7ba0e62a",
        ),
        "CoA-Custom-V2.0.3-Server.zip" => (
            97_128_649,
            "876c871ff616ee2f7fad6f096d70eec9aa4f05a1ebe6f6925bee522e8e7bd16a",
        ),
        "CoA-Custom-V2.0.3-Client-18.zip" => (
            57_277_519,
            "04e335984443505fea88fa07930c7ecc35eec3fe7fe19cdca8dc731a688f1896",
        ),
        _ => return Err(Error::Invalid("Unknown custom races archive.".into())),
    };
    Ok(crate::download::Job {
        url: url.into(),
        dest: cache.join(filename),
        sha256: sha256.into(),
        size,
    })
}

fn archive_matches(path: &Path, job: &crate::download::Job) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.len() == job.size)
        && fsx::sha256_file(path).is_ok_and(|hash| hash.eq_ignore_ascii_case(&job.sha256))
}

#[cfg(test)]
fn verify_archive(path: &Path, filename: &str) -> Result<()> {
    let job = archive_job(filename, "", Path::new(""))?;
    if !archive_matches(path, &job) {
        return Err(Error::Invalid(format!(
            "Custom races archive {filename} failed verification."
        )));
    }
    Ok(())
}

/// User-owned Downloads files are read-only candidates. Only the Manager's cache is repaired.
fn resolve_archive_with(
    transport: &dyn crate::download::Transport,
    job: &crate::download::Job,
    downloads: Option<&Path>,
    cancel: &Cancel,
    progress: &dyn Fn(crate::download::Progress),
) -> Result<PathBuf> {
    if cancel.is_set() {
        return Err(Error::Invalid("Download cancelled.".into()));
    }
    if let Some(downloads) = downloads {
        let candidate = downloads.join(job.dest.file_name().unwrap_or_default());
        if archive_matches(&candidate, job) {
            tracing::info!(path = %candidate.display(), "using verified archive in Downloads");
            progress(crate::download::Progress {
                downloaded: job.size,
                total: job.size,
                bytes_per_sec: 0,
            });
            return Ok(candidate);
        }
        if candidate.exists() {
            tracing::warn!(path = %candidate.display(), "ignoring incomplete or unverified archive in Downloads");
        }
    }

    let cache = job
        .dest
        .parent()
        .ok_or_else(|| Error::Invalid("Missing archive cache directory.".into()))?;
    fs::create_dir_all(cache)?;
    // Different installations share this cache; never append to or replace another active download.
    use fs4::fs_std::FileExt;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(suffix(&job.dest, "lock"))?;
    if !lock.try_lock_exclusive()? {
        return Err(Error::Invalid("Another custom races download is using this archive. Wait for it to finish, then retry.".into()));
    }
    if job.dest.is_file() {
        if archive_matches(&job.dest, job) {
            progress(crate::download::Progress {
                downloaded: job.size,
                total: job.size,
                bytes_per_sec: 0,
            });
            return Ok(job.dest.clone());
        }
        tracing::warn!(path = %job.dest.display(), "removing unverified Manager archive cache; downloading again");
        fs::remove_file(&job.dest)?;
    }
    // The shared downloader resumes .part files and verifies the complete archive before promotion.
    // A stale/corrupt prefix may only be detectable after resume: retry once from scratch in that case.
    match crate::download::fetch_with(transport, job, cancel, progress) {
        Err(Error::HashMismatch { .. }) if !cancel.is_set() => {
            tracing::warn!(path = %job.dest.display(), "archive verification failed; retrying a clean download");
            crate::download::fetch_with(transport, job, cancel, progress)?;
        }
        result => result?,
    }
    Ok(job.dest.clone())
}

/// The 1.9.2 file name of a V2 archive with the same bytes (same size and SHA-256 in both releases).
fn earlier_name(filename: &str) -> Option<&'static str> {
    match filename {
        "CoA-Custom-V2-Client-1.zip" => Some("CoA-Custom-1.9.2-Client-1.zip"),
        "CoA-Custom-V2-Client-2.zip" => Some("CoA-Custom-1.9.2-Client-2.zip"),
        "CoA-Custom-V2-Client-4.zip" => Some("CoA-Custom-1.9.2-Client-4.zip"),
        "CoA-Custom-V2-Client-5.zip" => Some("CoA-Custom-1.9.2-Client-5.zip"),
        _ => None,
    }
}

/// Locate a verified archive or resume downloading it into the Manager's cache.
fn resolve_or_download_archive(
    filename: &str,
    url: &str,
    transport: &dyn crate::download::Transport,
    cancel: &Cancel,
    report: &dyn Fn(&str, f32),
    base_percent: f32,
    percent_slice: f32,
) -> Result<PathBuf> {
    // Report before hashing existing files: otherwise checking a large archive looks like no operation.
    report(&format!("Checking {filename}..."), base_percent);
    let cache = std::env::temp_dir().join("coa-client-races");
    let downloads = std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Downloads"));
    let job = archive_job(filename, url, &cache)?;
    // V2 republished four 1.9.2 archives byte for byte under new names: reuse a verified 1.9.2 copy.
    if let Some(previous) = earlier_name(filename) {
        for folder in [Some(cache.as_path()), downloads.as_deref()].into_iter().flatten() {
            let candidate = folder.join(previous);
            if archive_matches(&candidate, &job) {
                tracing::info!(path = %candidate.display(), "using the identical 1.9.2 archive");
                return Ok(candidate);
            }
        }
    }
    resolve_archive_with(transport, &job, downloads.as_deref(), cancel, &|p| {
        let ratio = p.downloaded as f32 / p.total.max(1) as f32;
        report(
            &format!(
                "Downloading {filename} ({:.1} MB)...",
                p.downloaded as f64 / 1_048_576.0
            ),
            base_percent + ratio.clamp(0.0, 1.0) * percent_slice,
        );
    })
}

/// Download (if needed) and extract custom races client assets into the target WoW client.
pub fn install_client_patches(
    client_path: &Path,
    cancel: &Cancel,
    report: &dyn Fn(&str, f32),
) -> Result<()> {
    if !client_path.is_dir() {
        return Err(Error::Invalid(
            "Game client directory does not exist.".into(),
        ));
    }

    if crate::client::is_running(client_path) {
        return Err(Error::Invalid(
            "Close the game before installing custom races patches.".into(),
        ));
    }
    let _lock = patch_lock(client_path)?;
    recover_patch_transaction(client_path)?;
    let staging = tempfile::Builder::new()
        .prefix(".coa-races-stage-")
        .tempdir_in(client_path)?;
    let data_dir = staging.path().join("Data");
    fs::create_dir_all(&data_dir)?;

    // 2. Resolve/download archives (0.05 .. 0.50)
    let http = crate::download::HttpTransport::with_total_timeout(600)?;

    let mut resolved_archives = Vec::new();
    let n_archives = ARCHIVES.len() as f32;
    for (i, (fname, url)) in ARCHIVES.iter().enumerate() {
        let base_pct = 0.05 + (i as f32 / n_archives) * 0.45;
        let slice = 0.45 / n_archives;
        let archive_path =
            resolve_or_download_archive(fname, url, &http, cancel, report, base_pct, slice)?;
        resolved_archives.push(archive_path);
    }

    // 3. Extract archives (0.50 .. 0.95)
    let mut installed_mpqs = Vec::new();
    let mut installed_roots = Vec::new();

    for (i, arch_path) in resolved_archives.iter().enumerate() {
        if cancel.is_set() {
            return Err(Error::Invalid("Operation cancelled.".into()));
        }

        let arch_name = arch_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        report(
            &format!("Unpacking {arch_name}..."),
            0.50 + (i as f32 / n_archives) * 0.45,
        );

        let file = File::open(arch_path)?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| Error::Invalid(format!("Failed to open zip {arch_name}: {e}")))?;

        let num_files = zip.len();
        for file_idx in 0..num_files {
            if cancel.is_set() {
                return Err(Error::Invalid("Operation cancelled.".into()));
            }

            let mut zfile = zip
                .by_index(file_idx)
                .map_err(|e| Error::Invalid(format!("Zip entry error in {arch_name}: {e}")))?;

            if zfile.is_dir() {
                continue;
            }

            let zname = zfile.name().to_string();
            let zname_norm = zname.replace('\\', "/");

            if zname_norm.contains("files/client/") {
                let base = Path::new(&zname_norm)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();

                if base.to_ascii_lowercase().ends_with(".mpq") {
                    let dest = data_dir.join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    let disabled = data_dir.join(format!("{base}.disabled"));
                    if disabled.is_file() {
                        fs::remove_file(disabled)?;
                    }
                    installed_mpqs.push(base);
                } else if base.eq_ignore_ascii_case("dinput8.dll") {
                    let dest = staging.path().join(&base);
                    let mut out = File::create(&dest)?;
                    io::copy(&mut zfile, &mut out)?;
                    let disabled = staging.path().join(format!("{base}.disabled"));
                    if disabled.is_file() {
                        fs::remove_file(disabled)?;
                    }
                    installed_roots.push(base);
                }
            } else if let Some(relative) = zname_norm.split("files/addons/").nth(1) {
                if !RACE_ADDONS.contains(&relative) {
                    continue;
                }
                let relative = format!("Interface/AddOns/{relative}");
                let dest = fsx::safe_join(staging.path(), &relative)?;
                fs::create_dir_all(
                    dest.parent()
                        .ok_or_else(|| Error::Invalid("Addon file has no directory.".into()))?,
                )?;
                let mut out = File::create(&dest)?;
                io::copy(&mut zfile, &mut out)?;
                installed_roots.push(relative);
            } else if let Some(relative) = zname_norm.split("files/client_root/").nth(1) {
                installed_roots.push(stage_root_asset(staging.path(), relative, &mut zfile)?);
            }
        }
    }

    if !RACE_MPQS
        .iter()
        .all(|name| installed_mpqs.iter().any(|installed| installed == name))
    {
        return Err(Error::Invalid(
            "Custom races archives are missing required client patches.".into(),
        ));
    }

    // 4. Record manifest
    report("Finalizing manifest...", 0.98);
    let manifest_path = client_path.join(MANIFEST_FILENAME);
    installed_mpqs.sort();
    installed_mpqs.dedup();
    installed_roots.sort();
    installed_roots.dedup();

    let mut manifest_body = format!("# CoA Custom {CLIENT_VERSION}\n# Custom Race MPQ files\n");
    for m in &installed_mpqs {
        manifest_body.push_str(&format!("MPQ:{m}\n"));
    }
    manifest_body.push_str("# Custom Race root files\n");
    for r in &installed_roots {
        if let Some(relative) = r.strip_prefix("Interface/AddOns/") {
            manifest_body.push_str(&format!("ADDON:{relative}\n"));
        } else {
            manifest_body.push_str(&format!("ROOT:{r}\n"));
        }
    }

    let bundled: std::collections::BTreeMap<String, String> =
        serde_json::from_str(include_str!("custom_races_hashes.json"))?;
    let mut payload = Vec::new();
    for path in installed_mpqs
        .iter()
        .map(|name| format!("Data/{name}"))
        .chain(installed_roots.iter().cloned())
    {
        let staged = fsx::safe_join(staging.path(), &path)?;
        let hash = fsx::sha256_file(&staged)?;
        if bundled.get(&path) != Some(&hash) {
            return Err(Error::Invalid(format!(
                "Custom races file {path} failed verification."
            )));
        }
        manifest_body.push_str(&format!("SHA256:{hash}:{path}\n"));
        payload.push(PatchFile { path, sha256: hash });
    }
    if cancel.is_set() {
        return Err(Error::Invalid("Operation cancelled.".into()));
    }
    if payload.len() != bundled.len() {
        return Err(Error::Invalid(
            "Custom races archives are missing required runtime files.".into(),
        ));
    }
    let payload_paths: std::collections::BTreeSet<String> =
        payload.iter().map(|file| file.path.clone()).collect();
    let previous = patch_files(client_path)?;
    let mut tx = PatchTransaction::new(client_path)?;
    for file in payload {
        let dest = fsx::safe_join(client_path, &file.path)?;
        if dest.is_file()
            && !previous
                .iter()
                .any(|old| old.path == file.path && matches_patch(&dest, old))
            && !matches_patch(&dest, &file)
        {
            preserve_original(&mut tx, &dest)?;
        }
        tx.copy(&staging.path().join(&file.path), &dest)?;
        let disabled = suffix(&dest, "disabled");
        if disabled.is_file() {
            if !previous
                .iter()
                .any(|old| old.path == file.path && matches_patch(&disabled, old))
                && !matches_patch(&disabled, &file)
            {
                tx.move_file(
                    &disabled,
                    &suffix(
                        &disabled,
                        &format!("replaced-{}", uuid::Uuid::new_v4().simple()),
                    ),
                )?;
            } else {
                tx.save(&disabled)?;
            }
        }
    }
    retire_dropped_files(&mut tx, client_path, &previous, &payload_paths)?;
    tx.save(&manifest_path)?;
    fsx::atomic_write(&manifest_path, manifest_body.as_bytes())?;
    tx.finish()?;
    report("Custom races client patches installed successfully!", 1.0);
    tracing::info!(
        mpqs = installed_mpqs.len(),
        roots = installed_roots.len(),
        "custom races client patch installation complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn exercise_linked_addons(shared: &Path) {
        let client = tempfile::tempdir().unwrap();
        let root = client.path();
        fs::create_dir(root.join("Interface")).unwrap();
        let link = root.join("Interface/AddOns");
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(shared.to_string_lossy().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let name = "Interface/AddOns/CoAHideArmor/CoAHideArmor.lua";
        let active = root.join(name);
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        fs::write(shared.join("unrelated.lua"), b"user addon").unwrap();
        let staged = root.join("staged.lua");
        fs::write(&staged, b"custom").unwrap();
        let mut tx = PatchTransaction::new(root).unwrap();
        tx.copy(&staged, &active).unwrap();
        tx.finish().unwrap();
        record_files(root, &[name]);
        set_client_patch_enabled(root, false).unwrap();
        assert!(!active.exists());
        assert_eq!(fs::read(suffix(&active, "disabled")).unwrap(), b"custom");
        set_client_patch_enabled(root, true).unwrap();
        assert_eq!(fs::read(&active).unwrap(), b"custom");
        // Simulate process termination after a move, then recover twice.
        let mut tx = PatchTransaction::new(root).unwrap();
        tx.move_file(&active, &suffix(&active, "disabled")).unwrap();
        std::mem::forget(tx);
        recover_patch_transaction(root).unwrap();
        recover_patch_transaction(root).unwrap();
        assert_eq!(fs::read(&active).unwrap(), b"custom");
        assert!(!suffix(&active, "disabled").exists());
        // Replacement installation must preserve the old bytes on failure.
        {
            let mut tx = PatchTransaction::new(root).unwrap();
            fs::write(&staged, b"replacement").unwrap();
            tx.copy(&staged, &active).unwrap();
        }
        assert_eq!(fs::read(&active).unwrap(), b"custom");
        uninstall_client_patches(root).unwrap();
        assert!(!active.exists());
        assert_eq!(
            fs::read(shared.join("unrelated.lua")).unwrap(),
            b"user addon"
        );
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir(link).unwrap();
    }

    #[test]
    fn nested_root_manifest_remains_valid_and_rejects_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let relative = "Data/Retail/Assets/EquipmentFitting/60-0.bin";
        fs::write(
            temp.path().join(MANIFEST_FILENAME),
            format!("# CoA Custom {CLIENT_VERSION}\nROOT:{relative}\n"),
        )
        .unwrap();
        assert_eq!(patch_files(temp.path()).unwrap()[0].path, relative);
        fs::write(temp.path().join(MANIFEST_FILENAME), "ROOT:../outside.bin\n").unwrap();
        assert!(patch_files(temp.path()).is_err());
    }

    #[test]
    fn previous_split_release_retains_its_original_hash_inventory() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.8.3\n").unwrap();
        let old: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_8_3.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), old.len());
        assert!(files
            .iter()
            .all(|file| old.get(&file.path) == Some(&file.sha256)));
    }

    #[test]
    fn previous_191_release_keeps_its_hashes_and_does_not_claim_new_addons() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.9.1\n").unwrap();
        let old: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_9_1.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), old.len());
        assert!(files
            .iter()
            .all(|file| old.get(&file.path) == Some(&file.sha256)));
        assert!(!files.iter().any(|file| file.path.contains("CoAQuestSafe")));
    }

    #[test]
    fn root_assets_preserve_nested_paths_and_reject_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let relative = "Data/Retail/Assets/EquipmentFitting/60-0.bin";
        let result = stage_root_asset(temp.path(), relative, &mut &b"fitting"[..]).unwrap();
        assert_eq!(result, relative);
        assert_eq!(fs::read(temp.path().join(relative)).unwrap(), b"fitting");
        assert!(!temp.path().join("60-0.bin").exists());
        assert!(stage_root_asset(temp.path(), "../outside.bin", &mut &b"bad"[..]).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn shared_addons_junction_supports_install_switch_and_rollback() {
        let shared = tempfile::tempdir().unwrap();
        exercise_linked_addons(shared.path());
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "Set COA_TEST_OTHER_VOLUME to a writable directory on another volume"]
    fn shared_addons_on_another_volume_supports_install_switch_and_rollback() {
        let base = std::env::var_os("COA_TEST_OTHER_VOLUME").expect("COA_TEST_OTHER_VOLUME");
        let shared = tempfile::tempdir_in(base).unwrap();
        exercise_linked_addons(shared.path());
    }

    fn record_files(root: &Path, names: &[&str]) {
        let mut manifest = format!("# CoA Custom {CLIENT_VERSION}\n");
        for name in names {
            let (kind, base) = if let Some(base) = name.strip_prefix("Interface/AddOns/") {
                ("ADDON", base)
            } else {
                name.strip_prefix("Data/")
                    .filter(|base| !base.contains('/'))
                    .map_or(("ROOT", *name), |base| ("MPQ", base))
            };
            manifest.push_str(&format!(
                "{kind}:{base}\nSHA256:{}:{name}\n",
                fsx::sha256_file(&root.join(name)).unwrap()
            ));
        }
        fs::write(root.join(MANIFEST_FILENAME), manifest).unwrap();
    }

    const SWITCHED: &[&str] = &[
        "Data/patch-ZE1.MPQ",
        "dinput8.dll",
        "CoACustomRace.exe",
        "Start-CoACustomRace.bat",
        "Interface/AddOns/CoAHideArmor/CoAHideArmor.lua",
    ];
    const INERT: &[&str] = &[
        "Data/Retail/Cache/Retail/wow/decoded/2/strict/id-1.bin",
        "Data/Retail/RetailCasc.ini",
        "VulkanShaders/a.spv",
        "Vulkan/d3d9.dll",
        "forever_data/hd_forever_fog.txt",
        "EsteriaAppearance.bin",
    ];

    fn client_with(root: &Path, names: &[&str]) {
        for name in names {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, format!("custom {name}")).unwrap();
        }
        record_files(root, names);
    }

    #[test]
    fn disabling_hides_only_what_the_stock_client_loads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let all: Vec<&str> = SWITCHED.iter().chain(INERT).copied().collect();
        client_with(root, &all);
        set_client_patch_enabled(root, false).unwrap();
        for name in SWITCHED {
            assert!(!root.join(name).exists(), "{name} stays active");
            assert!(suffix(&root.join(name), "disabled").is_file(), "{name}");
        }
        for name in INERT {
            assert!(root.join(name).is_file(), "{name} was moved");
            assert!(!suffix(&root.join(name), "disabled").exists(), "{name}");
        }
        assert_eq!(client_patch_status(root).state, ClientPatchState::Disabled);
        set_client_patch_enabled(root, true).unwrap();
        assert_eq!(client_patch_status(root).state, ClientPatchState::Enabled);
        for name in &all {
            assert!(root.join(name).is_file(), "{name}");
        }
    }

    #[test]
    fn a_file_that_replaced_the_players_own_is_still_switched() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let name = "Data/Retail/RetailCasc.ini";
        client_with(root, &[name]);
        fs::write(suffix(&root.join(name), "original"), b"player").unwrap();
        set_client_patch_enabled(root, false).unwrap();
        assert_eq!(fs::read(root.join(name)).unwrap(), b"player");
        assert!(suffix(&root.join(name), "disabled").is_file());
        assert_eq!(client_patch_status(root).state, ClientPatchState::Disabled);
    }

    #[test]
    fn a_client_disabled_by_an_older_manager_is_disabled_and_enables() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let all: Vec<&str> = SWITCHED.iter().chain(INERT).copied().collect();
        client_with(root, &all);
        // 0.9.13 renamed every file of the payload.
        for name in &all {
            fs::rename(root.join(name), suffix(&root.join(name), "disabled")).unwrap();
        }
        assert_eq!(client_patch_status(root).state, ClientPatchState::Disabled);
        set_client_patch_enabled(root, false).unwrap();
        for name in INERT {
            assert!(root.join(name).is_file(), "{name} not restored");
        }
        assert_eq!(client_patch_status(root).state, ClientPatchState::Disabled);
        set_client_patch_enabled(root, true).unwrap();
        assert_eq!(client_patch_status(root).state, ClientPatchState::Enabled);
    }

    #[test]
    fn a_transaction_left_by_an_older_manager_is_recovered() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let directory = root.join(".coa-races-transaction");
        fs::create_dir(&directory).unwrap();
        // The source was moved into the journal and then on to its destination when the old Manager stopped.
        fs::write(root.join("patch.MPQ.disabled"), b"patch bytes").unwrap();
        fs::write(
            directory.join(LEGACY_JOURNAL),
            r#"[{"path":"patch.MPQ.disabled","existed":false},{"path":"patch.MPQ","existed":true,"moved_to":"patch.MPQ.disabled"}]"#,
        )
        .unwrap();
        recover_patch_transaction(root).unwrap();
        assert_eq!(fs::read(root.join("patch.MPQ")).unwrap(), b"patch bytes");
        assert!(!root.join("patch.MPQ.disabled").exists());
        assert!(!directory.exists());
    }

    #[test]
    fn a_journal_line_cut_short_by_a_crash_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let file = root.join("dinput8.dll");
        fs::write(&file, b"previous").unwrap();
        let mut tx = PatchTransaction::new(root).unwrap();
        tx.save(&file).unwrap();
        fs::write(&file, b"replacement").unwrap();
        drop(tx.journal.take());
        std::mem::forget(tx);
        let journal = root.join(".coa-races-transaction").join(JOURNAL);
        let mut bytes = fs::read(&journal).unwrap();
        bytes.extend_from_slice(br#"{"saved":{"path":"other.dll","exi"#);
        fs::write(&journal, bytes).unwrap();
        recover_patch_transaction(root).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"previous");
        assert!(!root.join(".coa-races-transaction").exists());
    }

    #[test]
    fn switching_a_release_sized_payload_takes_seconds() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut names: Vec<String> = (0..7000)
            .map(|i| format!("VulkanShaders/shader-{i}.spv"))
            .collect();
        names.extend((0..80).map(|i| format!("Data/patch-Z{i}.MPQ")));
        names.extend((0..140).map(|i| format!("Interface/AddOns/Addon{}/file{i}.lua", i % 9)));
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        client_with(root, &names);
        let started = std::time::Instant::now();
        set_client_patch_enabled(root, false).unwrap();
        set_client_patch_enabled(root, true).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(client_patch_status(root).state, ClientPatchState::Enabled);
        // 0.9.13 needed minutes here: it moved every file and rewrote the whole journal after each one.
        assert!(elapsed < std::time::Duration::from_secs(60), "{elapsed:?}");
    }

    #[test]
    fn an_update_retires_files_the_new_release_no_longer_ships() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("Ascension.exe"), b"1.9.2 client").unwrap();
        fs::write(root.join("Ascension.exe.original"), b"player's own client").unwrap();
        fs::write(root.join("Wow.exe"), b"changed by the player").unwrap();
        let ours = |path: &str, bytes: &[u8]| PatchFile {
            path: path.into(),
            sha256: hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes)),
        };
        let previous = vec![ours("Ascension.exe", b"1.9.2 client"), ours("Wow.exe", b"old package bytes")];
        let mut tx = PatchTransaction::new(root).unwrap();
        retire_dropped_files(&mut tx, root, &previous, &Default::default()).unwrap();
        tx.finish().unwrap();
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), b"player's own client");
        assert_eq!(fs::read(root.join("Wow.exe")).unwrap(), b"changed by the player");
    }

    #[test]
    #[ignore = "Requires the downloaded release archives"]
    fn bundled_hashes_match_verified_release_archives() {
        use sha2::{Digest, Sha256};
        let folder = PathBuf::from(
            std::env::var_os("COA_RACES_ARCHIVE_DIR").expect("COA_RACES_ARCHIVE_DIR"),
        );
        let expected: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let mut actual = std::collections::BTreeMap::new();
        for (name, _) in ARCHIVES {
            let path = folder.join(name);
            verify_archive(&path, name).unwrap();
            let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).unwrap();
                let name = entry.name().replace('\\', "/");
                if entry.is_dir() {
                    continue;
                }
                let relative = if let Some(name) = name.split("files/client_root/").nth(1) {
                    name.to_string()
                } else if let Some(name) = name.split("files/client/").nth(1) {
                    if name == "dinput8.dll" {
                        name.to_string()
                    } else {
                        format!("Data/{name}")
                    }
                } else if let Some(name) = name.split("files/addons/").nth(1) {
                    if !RACE_ADDONS.contains(&name) {
                        continue;
                    }
                    format!("Interface/AddOns/{name}")
                } else {
                    continue;
                };
                let mut hash = Sha256::new();
                let mut buffer = vec![0; 1024 * 1024];
                loop {
                    let read = entry.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    hash.update(&buffer[..read]);
                }
                actual.insert(relative, hex::encode(hash.finalize()));
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    #[ignore = "Requires COA_UPGRADE_CLIENT: a disposable client with 1.9.2 installed by Manager 0.9.12"]
    fn an_installed_1_9_2_client_updates_to_v2_and_gets_its_own_executable_back() {
        let root = PathBuf::from(std::env::var_os("COA_UPGRADE_CLIENT").expect("COA_UPGRADE_CLIENT"));
        assert_eq!(client_patch_status(&root).version.as_deref(), Some("1.9.2"));
        let patched = fs::read(root.join("Ascension.exe")).unwrap();
        let original = fs::read(root.join("Ascension.exe.original")).unwrap();
        assert_ne!(patched, original);
        install_client_patches(&root, &Cancel::default(), &|_, _| {}).unwrap();
        let status = client_patch_status(&root);
        assert_eq!(status.version.as_deref(), Some(CLIENT_VERSION));
        assert_eq!(status.state, ClientPatchState::Enabled);
        assert!(root.join("CoACustomRace.exe").is_file());
        assert!(root.join("Data/patch-ZZZZZY.MPQ").is_file());
        // 1.9.2's patched Ascension.exe is retired and the player's own executable is back.
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), original);
        uninstall_client_patches(&root).unwrap();
        assert_eq!(client_patch_status(&root).state, ClientPatchState::NotInstalled);
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), original);
        assert!(!root.join("CoACustomRace.exe").exists());
    }

    #[test]
    #[ignore = "Requires the downloaded release archives in the normal download cache"]
    fn release_archives_install_toggle_and_uninstall_in_a_disposable_client() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("Data")).unwrap();
        fs::write(root.join("Ascension.exe"), b"original game executable").unwrap();
        fs::write(root.join("Data/patch-T.MPQ"), b"original game patch").unwrap();
        fs::write(root.join("Data/patch-user.MPQ"), b"unrelated user patch").unwrap();
        install_client_patches(root, &Cancel::default(), &|_, _| {}).unwrap();
        let status = client_patch_status(root);
        assert_eq!(status.version.as_deref(), Some(CLIENT_VERSION));
        assert_eq!(status.state, ClientPatchState::Enabled);
        assert!(root.join("Data/patch-ZZZD.MPQ").is_file());
        assert!(root
            .join("Interface/AddOns/CoAHideArmor/CoAHideArmor.lua")
            .is_file());
        // V2: its own client next to the untouched game executable, and the server package's addons.
        assert!(root.join("CoACustomRace.exe").is_file());
        assert!(root.join("Vulkan/d3d9.dll").is_file());
        assert_eq!(fs::read(root.join("Ascension.exe")).unwrap(), b"original game executable");
        assert!(root
            .join("Interface/AddOns/Ascension_Subclass")
            .is_dir());
        let started = std::time::Instant::now();
        set_client_patch_enabled(root, false).unwrap();
        eprintln!("disable took {:?}", started.elapsed());
        assert_eq!(client_patch_status(root).state, ClientPatchState::Disabled);
        // Only what the stock client loads is hidden; the data its executable alone reads stays put.
        assert!(!root.join("CoACustomRace.exe").exists());
        assert!(!root.join("dinput8.dll").exists());
        assert!(!root.join("Data/patch-ZZZD.MPQ").exists());
        assert!(!root.join("Interface/AddOns/CoAHideArmor/CoAHideArmor.lua").exists());
        assert!(root.join("Vulkan/d3d9.dll").is_file());
        assert!(root.join("Data/Retail/RetailCasc.ini").is_file());
        let hidden = patch_files(root)
            .unwrap()
            .iter()
            .filter(|file| suffix(&root.join(&file.path), "disabled").is_file())
            .count();
        eprintln!("hidden {hidden} of {}", patch_files(root).unwrap().len());
        assert_eq!(
            fs::read(root.join("Ascension.exe")).unwrap(),
            b"original game executable"
        );
        assert_eq!(
            fs::read(root.join("Data/patch-T.MPQ")).unwrap(),
            b"original game patch"
        );
        let started = std::time::Instant::now();
        set_client_patch_enabled(root, true).unwrap();
        eprintln!("enable took {:?}", started.elapsed());
        assert_eq!(client_patch_status(root).state, ClientPatchState::Enabled);
        uninstall_client_patches(root).unwrap();
        assert_eq!(
            client_patch_status(root).state,
            ClientPatchState::NotInstalled
        );
        assert_eq!(
            fs::read(root.join("Ascension.exe")).unwrap(),
            b"original game executable"
        );
        assert_eq!(
            fs::read(root.join("Data/patch-T.MPQ")).unwrap(),
            b"original game patch"
        );
        assert_eq!(
            fs::read(root.join("Data/patch-user.MPQ")).unwrap(),
            b"unrelated user patch"
        );
        assert!(!root.join("Data/patch-ZZZD.MPQ").exists());
        assert!(!root.join("CoACustomRace.exe").exists());
    }

    #[test]
    fn legacy_installation_uses_its_own_hashes_when_preserving_originals() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.5\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_5.json")).unwrap();
        let current: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(
            files
                .iter()
                .find(|file| file.path == "Ascension.exe")
                .unwrap()
                .sha256,
            legacy["Ascension.exe"]
        );
        assert_eq!(files.len(), legacy.len());
        assert!(current.len() > legacy.len());
        assert!(!files.iter().any(|file| file.path == "Data/patch-ZZS.MPQ"));
    }

    #[test]
    fn previous_release_uses_its_own_hashes_for_upgrade_and_removal() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.6\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_6.json")).unwrap();
        let current: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), legacy.len());
        assert_eq!(
            files
                .iter()
                .find(|file| file.path == "Data/patch-T.MPQ")
                .unwrap()
                .sha256,
            legacy["Data/patch-T.MPQ"]
        );
        assert_ne!(legacy["Data/patch-T.MPQ"], current["Data/patch-T.MPQ"]);
        assert!(!files.iter().any(|file| file.path == "Data/patch-ZZW.MPQ"));
        assert!(legacy
            .keys()
            .filter(|key| !RETIRED_FILES.contains(&key.as_str()))
            .all(|key| current.contains_key(key)));
    }

    #[test]
    fn previous_appearance_release_keeps_its_hashes_and_requests_an_update() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.8\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_8.json")).unwrap();
        let current: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), legacy.len());
        for file in files {
            assert_eq!(file.sha256, legacy[&file.path]);
        }
        assert!(legacy
            .keys()
            .filter(|key| !RETIRED_FILES.contains(&key.as_str()))
            .all(|key| current.contains_key(key)));
        assert!(!legacy.contains_key("Data/patch-ZZZD.MPQ"));
        assert!(current.contains_key("Data/patch-ZZZD.MPQ"));
        let status = client_patch_status(temp.path());
        assert_eq!(status.version.as_deref(), Some("1.8"));
        assert_eq!(status.state, ClientPatchState::PartiallyInstalled);
    }

    #[test]
    fn previous_racial_release_keeps_its_hashes_and_addon_paths_stay_inside_client() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.7\n").unwrap();
        let legacy: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("custom_races_hashes_1_7.json")).unwrap();
        let files = patch_files(temp.path()).unwrap();
        assert_eq!(files.len(), legacy.len());
        assert_eq!(
            files
                .iter()
                .find(|file| file.path == "Data/patch-ZZW.MPQ")
                .unwrap()
                .sha256,
            legacy["Data/patch-ZZW.MPQ"]
        );
        assert!(!files.iter().any(|file| file.path == "Data/patch-ZZZ.MPQ"));
        fs::write(
            temp.path().join(MANIFEST_FILENAME),
            "# CoA Custom 1.8\nADDON:CoAWC2Cursor/CoAWC2Cursor.lua\n",
        )
        .unwrap();
        assert_eq!(
            patch_files(temp.path()).unwrap()[0].path,
            "Interface/AddOns/CoAWC2Cursor/CoAWC2Cursor.lua"
        );
        fs::write(
            temp.path().join(MANIFEST_FILENAME),
            "ADDON:../../../outside\n",
        )
        .unwrap();
        assert!(patch_files(temp.path()).is_err());
    }

    #[test]
    fn another_patch_operation_cannot_enter_the_same_client() {
        let temp = tempfile::tempdir().unwrap();
        let first = patch_lock(temp.path()).unwrap();
        assert!(patch_lock(temp.path()).is_err());
        drop(first);
        patch_lock(temp.path()).unwrap();
    }

    #[test]
    fn switching_preserves_launcher_updates_for_every_owned_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        let names = [
            "Data/patch-T.MPQ",
            "Data/patch-ZE1.MPQ",
            "Ascension.exe",
            "dinput8.dll",
            "EsteriaAppearance.dll",
        ];
        for name in names {
            fs::write(root.join(name), b"custom").unwrap();
            fs::write(suffix(&root.join(name), "original"), b"old official").unwrap();
        }
        record_files(root, &names);
        set_client_patch_enabled(root, false).unwrap();
        for name in names {
            assert_eq!(fs::read(root.join(name)).unwrap(), b"old official");
            fs::write(root.join(name), b"new official").unwrap();
        }
        set_client_patch_enabled(root, true).unwrap();
        for name in names {
            assert_eq!(fs::read(root.join(name)).unwrap(), b"custom");
            assert_eq!(
                fs::read(suffix(&root.join(name), "original")).unwrap(),
                b"new official"
            );
        }
        // An official launcher can also replace a currently enabled file.
        fs::write(root.join("Ascension.exe"), b"newest official").unwrap();
        assert_eq!(
            client_patch_status(root).state,
            ClientPatchState::PartiallyInstalled
        );
        set_client_patch_enabled(root, false).unwrap();
        assert_eq!(
            fs::read(root.join("Ascension.exe")).unwrap(),
            b"newest official"
        );
        assert!(set_client_patch_enabled(root, true)
            .unwrap_err()
            .to_string()
            .contains("Repair"));
        uninstall_client_patches(root).unwrap();
        assert_eq!(
            fs::read(root.join("Ascension.exe")).unwrap(),
            b"newest official"
        );
        assert_eq!(
            client_patch_status(root).state,
            ClientPatchState::NotInstalled
        );
    }

    #[test]
    fn failed_and_interrupted_transactions_restore_previous_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let file = root.join("Ascension.exe");
        fs::write(&file, b"previous").unwrap();
        {
            let mut tx = PatchTransaction::new(root).unwrap();
            tx.save(&file).unwrap();
            fs::write(&file, b"replacement").unwrap();
            let staged = root.join("staged.dll");
            fs::write(&staged, b"new").unwrap();
            tx.copy(&staged, &root.join("Data/new.dll")).unwrap();
            assert_eq!(fs::read(root.join("Data/new.dll")).unwrap(), b"new");
        }
        assert_eq!(fs::read(&file).unwrap(), b"previous");
        assert!(!root.join("Data/new.dll").exists());
        let mut tx = PatchTransaction::new(root).unwrap();
        tx.save(&file).unwrap();
        fs::write(&file, b"interrupted").unwrap();
        std::mem::forget(tx);
        recover_patch_transaction(root).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"previous");
    }

    #[test]
    fn interrupted_patch_moves_restore_source_and_destination() {
        for existing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let source = root.join("patch.MPQ");
            let target = root.join("patch.MPQ.disabled");
            fs::write(&source, b"patch bytes").unwrap();
            if existing {
                fs::write(&target, b"previous destination").unwrap();
            }
            let mut tx = PatchTransaction::new(root).unwrap();
            tx.move_file(&source, &target).unwrap();
            assert!(!source.exists());
            assert_eq!(fs::read(&target).unwrap(), b"patch bytes");
            // The source is moved, not copied into both the journal and destination.
            assert!(!root.join(".coa-races-transaction/1").exists());
            std::mem::forget(tx);
            recover_patch_transaction(root).unwrap();
            recover_patch_transaction(root).unwrap();
            assert_eq!(fs::read(&source).unwrap(), b"patch bytes");
            if existing {
                assert_eq!(fs::read(&target).unwrap(), b"previous destination");
            } else {
                assert!(!target.exists());
            }
        }
    }

    #[test]
    fn existing_backups_and_foreign_disabled_files_are_retained() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let file = root.join("dinput8.dll");
        fs::write(&file, b"custom").unwrap();
        record_files(root, &["dinput8.dll"]);
        fs::write(suffix(&file, "disabled"), b"third party").unwrap();
        fs::write(suffix(&file, "original"), b"original").unwrap();
        set_client_patch_enabled(root, false).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert!(fs::read_dir(root)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains("replaced-")));
        fs::write(&file, b"updated original").unwrap();
        set_client_patch_enabled(root, true).unwrap();
        assert!(fs::read_dir(root).unwrap().flatten().any(|entry| entry
            .file_name()
            .to_string_lossy()
            .contains("original.previous-")));
    }

    #[test]
    fn uninstall_moves_corrupted_custom_only_assets_out_of_the_active_client() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        let file = root.join("Data/patch-ZE1.MPQ");
        fs::write(&file, b"custom").unwrap();
        record_files(root, &["Data/patch-ZE1.MPQ"]);
        fs::write(&file, b"corrupted custom").unwrap();
        uninstall_client_patches(root).unwrap();
        assert!(!file.exists());
        assert!(fs::read_dir(root.join("Data"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains("replaced-")));
        assert_eq!(
            client_patch_status(root).state,
            ClientPatchState::NotInstalled
        );
    }

    #[test]
    fn unknown_or_corrupt_bytes_are_not_reported_as_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Data")).unwrap();
        for name in RACE_MPQS {
            fs::write(root.join("Data").join(name), b"corrupt").unwrap();
        }
        fs::write(root.join("dinput8.dll"), b"corrupt").unwrap();
        fs::write(
            root.join(MANIFEST_FILENAME),
            format!("# CoA Custom {CLIENT_VERSION}\n"),
        )
        .unwrap();
        assert_eq!(
            client_patch_status(root).state,
            ClientPatchState::PartiallyInstalled
        );
    }

    #[test]
    fn an_old_complete_installation_requires_an_update() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Data")).unwrap();
        for name in RACE_MPQS {
            fs::write(temp.path().join("Data").join(name), b"old patch").unwrap();
        }
        fs::write(temp.path().join("dinput8.dll"), b"old runtime").unwrap();
        fs::write(temp.path().join(MANIFEST_FILENAME), "# CoA Custom 1.4\n").unwrap();
        assert_eq!(
            client_patch_status(temp.path()).state,
            ClientPatchState::PartiallyInstalled
        );
    }

    #[test]
    fn an_unpatched_client_is_left_untouched() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Data")).unwrap();
        fs::write(temp.path().join("Data/patch-T.MPQ"), b"original").unwrap();
        set_client_patch_enabled(temp.path(), false).unwrap();
        assert_eq!(
            fs::read(temp.path().join("Data/patch-T.MPQ")).unwrap(),
            b"original"
        );
    }

    struct ArchiveTransport {
        body: Vec<u8>,
        ranges: std::sync::Mutex<Vec<u64>>,
    }

    impl crate::download::Transport for ArchiveTransport {
        fn get(&self, _: &str, from: u64) -> std::result::Result<crate::download::Reply, String> {
            self.ranges.lock().unwrap().push(from);
            Ok(crate::download::Reply {
                status: if from == 0 { 200 } else { 206 },
                content_range: (from > 0)
                    .then(|| format!("bytes {from}-{}/{}", self.body.len() - 1, self.body.len())),
                body: Box::new(io::Cursor::new(self.body[from as usize..].to_vec())),
            })
        }
    }

    fn archive_fixture(root: &Path) -> (crate::download::Job, ArchiveTransport) {
        use sha2::{Digest, Sha256};
        let body = b"verified archive bytes for recovery tests".to_vec();
        let job = crate::download::Job {
            url: "https://example.invalid/patch.zip".into(),
            dest: root.join("cache/patch.zip"),
            sha256: hex::encode(Sha256::digest(&body)),
            size: body.len() as u64,
        };
        fs::create_dir_all(job.dest.parent().unwrap()).unwrap();
        (
            job,
            ArchiveTransport {
                body,
                ranges: Default::default(),
            },
        )
    }

    #[test]
    fn download_uses_verified_user_archive_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        let downloads = temp.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let archive = downloads.join("patch.zip");
        fs::write(&archive, &transport.body).unwrap();
        let path = resolve_archive_with(
            &transport,
            &job,
            Some(&downloads),
            &Cancel::default(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(path, archive);
        assert!(transport.ranges.lock().unwrap().is_empty());
        assert!(!job.dest.exists());
    }

    #[test]
    fn download_skips_bad_user_archive_and_repairs_bad_cache() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        let downloads = temp.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let bad = vec![b'x'; transport.body.len()];
        fs::write(downloads.join("patch.zip"), &bad).unwrap();
        fs::write(downloads.join("patch.zip.part"), b"user browser download").unwrap();
        fs::write(&job.dest, &bad).unwrap();
        let path = resolve_archive_with(
            &transport,
            &job,
            Some(&downloads),
            &Cancel::default(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(path, job.dest);
        assert_eq!(fs::read(&path).unwrap(), transport.body);
        assert_eq!(fs::read(downloads.join("patch.zip")).unwrap(), bad);
        assert_eq!(
            fs::read(downloads.join("patch.zip.part")).unwrap(),
            b"user browser download"
        );
        assert_eq!(*transport.ranges.lock().unwrap(), vec![0]);
    }

    #[test]
    fn download_resumes_leftover_part_and_verifies_complete_archive() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        let part = suffix(&job.dest, "part");
        fs::write(&part, &transport.body[..9]).unwrap();
        resolve_archive_with(&transport, &job, None, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(*transport.ranges.lock().unwrap(), vec![9]);
        assert_eq!(fs::read(&job.dest).unwrap(), transport.body);
        assert!(!part.exists());
    }

    #[test]
    fn download_reuses_good_cache_even_when_user_archive_is_incomplete() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        let downloads = temp.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        fs::write(downloads.join("patch.zip"), b"incomplete").unwrap();
        fs::write(&job.dest, &transport.body).unwrap();
        let path = resolve_archive_with(
            &transport,
            &job,
            Some(&downloads),
            &Cancel::default(),
            &|_| {},
        )
        .unwrap();
        assert_eq!(path, job.dest);
        assert!(transport.ranges.lock().unwrap().is_empty());
        assert_eq!(
            fs::read(downloads.join("patch.zip")).unwrap(),
            b"incomplete"
        );
    }

    #[test]
    fn download_restarts_once_when_resumed_prefix_is_corrupt() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        fs::write(suffix(&job.dest, "part"), b"bad").unwrap();
        resolve_archive_with(&transport, &job, None, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(*transport.ranges.lock().unwrap(), vec![3, 0]);
        assert_eq!(fs::read(&job.dest).unwrap(), transport.body);
    }

    #[test]
    fn download_promotes_completed_part_without_redownloading() {
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        fs::write(suffix(&job.dest, "part"), &transport.body).unwrap();
        resolve_archive_with(&transport, &job, None, &Cancel::default(), &|_| {}).unwrap();
        assert!(transport.ranges.lock().unwrap().is_empty());
        assert_eq!(fs::read(&job.dest).unwrap(), transport.body);
    }

    #[test]
    fn download_stops_after_one_clean_retry_if_server_bytes_are_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let (job, mut transport) = archive_fixture(temp.path());
        transport.body.fill(b'x');
        let result = resolve_archive_with(&transport, &job, None, &Cancel::default(), &|_| {});
        assert!(matches!(result, Err(Error::HashMismatch { .. })));
        assert_eq!(*transport.ranges.lock().unwrap(), vec![0, 0]);
        assert!(!job.dest.exists());
        assert!(!suffix(&job.dest, "part").exists());
    }

    #[test]
    fn download_does_not_modify_an_archive_owned_by_another_download() {
        use fs4::fs_std::FileExt;
        let temp = tempfile::tempdir().unwrap();
        let (job, transport) = archive_fixture(temp.path());
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(suffix(&job.dest, "lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        fs::write(suffix(&job.dest, "part"), b"active download").unwrap();
        assert!(resolve_archive_with(&transport, &job, None, &Cancel::default(), &|_| {}).is_err());
        assert_eq!(
            fs::read(suffix(&job.dest, "part")).unwrap(),
            b"active download"
        );
        assert!(transport.ranges.lock().unwrap().is_empty());
    }

    #[test]
    fn test_patch_status_and_toggle() {
        let temp = tempfile::tempdir().unwrap();
        let client_path = temp.path();
        let data_dir = client_path.join("Data");
        fs::create_dir_all(&data_dir).unwrap();

        // 1. Initial state: Not installed
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::NotInstalled);

        fs::write(
            client_path.join(MANIFEST_FILENAME),
            format!("# CoA Custom {CLIENT_VERSION}\n"),
        )
        .unwrap();
        // 2. Create MPQs and root files
        for mpq in RACE_MPQS {
            fs::write(data_dir.join(mpq), b"test mpq content").unwrap();
        }
        fs::write(client_path.join("dinput8.dll"), b"test dll").unwrap();
        fs::write(client_path.join("EsteriaAppearance.dll"), b"test dll").unwrap();

        let names: Vec<String> = RACE_MPQS
            .iter()
            .map(|name| format!("Data/{name}"))
            .chain(["dinput8.dll".into(), "EsteriaAppearance.dll".into()])
            .collect();
        record_files(
            client_path,
            &names.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Enabled);
        assert_eq!(st.installed_mpqs, RACE_MPQS.len());

        // 3. Disable patches
        set_client_patch_enabled(client_path, false).unwrap();
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Disabled);
        assert!(data_dir.join("patch-T.MPQ.disabled").is_file());
        assert!(!data_dir.join("patch-T.MPQ").is_file());
        assert!(client_path.join("dinput8.dll.disabled").is_file());
        assert!(!client_path.join("dinput8.dll").is_file());

        // 4. Re-enable patches
        set_client_patch_enabled(client_path, true).unwrap();
        let st = client_patch_status(client_path);
        assert_eq!(st.state, ClientPatchState::Enabled);
        assert!(data_dir.join("patch-T.MPQ").is_file());
        assert!(!data_dir.join("patch-T.MPQ.disabled").is_file());
        assert!(client_path.join("dinput8.dll").is_file());
        assert!(!client_path.join("dinput8.dll.disabled").is_file());
    }
    #[test]
    fn disabling_bot_custom_races_excludes_every_nonstandard_race() {
        let temp = tempfile::tempdir().unwrap();
        let conf = temp.path().join("Core/configs/modules/playerbots.conf");
        fs::create_dir_all(conf.parent().unwrap()).unwrap();
        fs::write(&conf, "AiPlayerbot.Enabled = 1\n").unwrap();
        sync_bot_races_config(temp.path(), false).unwrap();
        assert_eq!(is_bot_custom_races_allowed(temp.path()), Some(false));
        let parsed =
            crate::config::parser::ConfFile::parse_bytes(&fs::read(conf).unwrap()).unwrap();
        let value = parsed
            .get("AiPlayerbot.ExcludedBotRaces")
            .unwrap()
            .trim_matches('"');
        let excluded: Vec<u32> = value.split(',').map(|race| race.parse().unwrap()).collect();
        for race in 1..=128 {
            assert_eq!(
                excluded.contains(&race),
                ![1, 2, 3, 4, 5, 6, 7, 8, 10, 11].contains(&race),
                "race {race}"
            );
        }
    }

    #[test]
    fn bot_race_sync_preserves_user_and_crashing_exclusions() {
        let temp = tempfile::tempdir().unwrap();
        let conf = temp.path().join("Core/configs/modules/playerbots.conf");
        fs::create_dir_all(conf.parent().unwrap()).unwrap();
        for value in ["", "1,82,97", EXCLUDED_BOT_RACES_RECOMMENDED] {
            let original = format!("AiPlayerbot.ExcludedBotRaces = \"{value}\"\nAiPlayerbot.CrashingBotRaces = \"72,77,97\"\n");
            fs::write(&conf, &original).unwrap();
            sync_bot_races_config(temp.path(), true).unwrap();
            assert_eq!(fs::read_to_string(&conf).unwrap(), original);
            sync_bot_races_config(temp.path(), false).unwrap();
            assert_eq!(fs::read_to_string(&conf).unwrap(), original);
        }
    }
}
