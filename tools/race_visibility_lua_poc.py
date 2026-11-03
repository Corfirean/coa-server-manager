"""Version-pinned offline experiment; never installs files or launches the game."""

import hashlib


SOURCE_SHA256 = "8816ef9b6839652db60da6de7d2a6d09ceed1f80323ca5c8e55271a4d5aacac3"
PROBE_IDS = {82, 83, 97}


def build_probe(source, disabled):
    if hashlib.sha256(source).hexdigest() != SOURCE_SHA256:
        raise ValueError("Unsupported CharacterCreate.lua fingerprint")
    disabled = set(disabled)
    if not disabled <= PROBE_IDS:
        raise ValueError("This experiment supports only previously verified probe RaceIDs")
    text = source.decode("utf-8").replace("\r", "")
    prefix = "local CoaManagedDisabled = {" + ",".join(f"[{race}]=true" for race in sorted(disabled)) + "}\n"
    prefix += """local function CoaManagedRaceDisabled(raceId)
    return raceId ~= nil and CoaManagedDisabled[raceId] == true
end
local function CoaManagedDisabledLabel()
    local labels = {
        enUS = "Disabled by server", enGB = "Disabled by server",
        ruRU = "Отключено сервером", deDE = "Vom Server deaktiviert",
        frFR = "Désactivé par le serveur", esES = "Desactivada por el servidor",
        esMX = "Desactivada por el servidor", zhCN = "服务器已禁用"
    }
    return labels[GetLocale and GetLocale() or "enUS"] or labels.enUS
end
"""
    replacements = [
        ('local hide = hidden["#"..i] or hidden[name] or hidden[strlower(name or "")]',
         'local hide = CoaManagedRaceDisabled(button.bonusRaceID) or hidden["#"..i] or hidden[name] or hidden[strlower(name or "")]'),
        ('\t\t\t\tbutton.coaUserHidden = nil\n\t\t\t\tbutton:Show()\n',
         '\t\t\t\tbutton.coaUserHidden = CoaManagedRaceDisabled(button.bonusRaceID) or nil\n'
         '\t\t\t\tif not button.coaUserHidden then button:Show() else button:Hide() end\n'),
        ("tinsert(rows, { id = i, raceID = raceID, name = name, faction = (ok and faction) or 'Alliance' })",
         "tinsert(rows, { id = i, raceID = raceID, policyRaceID = button.bonusRaceID, name = name, faction = (ok and faction) or 'Alliance' })"),
        ('if not f.draft or not self.raceName then',
         'if self.coaServerDisabled or not f.draft or not self.raceName then'),
        ("\t\t\t\trow.label:SetText((info.faction == 'Horde' and '|cffff2020' or '|cff00a0ff')..info.name..' |cffaaa37a(ID '..info.raceID..')|r')\n"
         "\t\t\t\tlocal hidden = f.draft and (f.draft['#'..info.id] or f.draft[info.name] or f.draft[strlower(info.name)])\n"
         "\t\t\t\trow:SetChecked(not hidden)",
         """                row.coaServerDisabled = CoaManagedRaceDisabled(info.policyRaceID)
                local hidden = f.draft and (f.draft['#'..info.id] or f.draft[info.name] or f.draft[strlower(info.name)])
                if row.coaServerDisabled then
                    row.label:SetText('|cff808080'..info.name..' (ID '..info.raceID..') - '..CoaManagedDisabledLabel()..'|r')
                    row:Disable()
                else
                    row.label:SetText((info.faction == 'Horde' and '|cffff2020' or '|cff00a0ff')..info.name..' |cffaaa37a(ID '..info.raceID..')|r')
                    row:Enable()
                end
                row:SetChecked(not row.coaServerDisabled and not hidden)"""),
    ]
    for old, new in replacements:
        if text.count(old) != 1:
            raise ValueError("Expected exactly one known Lua integration point")
        text = text.replace(old, new)
    return (prefix + text).encode("utf-8")
