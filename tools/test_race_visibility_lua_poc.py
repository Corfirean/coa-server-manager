import importlib.util
import os
from pathlib import Path
import unittest

from race_visibility_lua_poc import build_probe


@unittest.skipUnless(os.environ.get("COA_TEST_CHARACTER_CREATE_LUA") and importlib.util.find_spec("lupa"),
                     "Requires an explicitly selected extracted Lua and installed Lua 5.1 runtime")
class ManagedFilterProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        from lupa.lua51 import LuaRuntime
        cls.runtime_type = LuaRuntime
        cls.source = Path(os.environ["COA_TEST_CHARACTER_CREATE_LUA"]).read_bytes()
        cls.generated = build_probe(cls.source, [82, 83, 97]).decode()
        cls.prefix = cls.generated[:cls.generated.index("CHAR_CREATE_HELP_NEXT_BUTTON")]

    def rows(self, race=82, personal=False, locale="enUS"):
        lua = self.runtime_type(unpack_returned_tuples=True)
        lua.execute("""
            F = {offset=0, entries={}, draft={}, rows={}}
            strlower = string.lower
            function makeRow()
                local row={label={}}
                row.label.SetText=function(self,value) self.text=value end
                row.SetChecked=function(self,value) self.checked=value end
                row.Disable=function(self) self.enabled=false end
                row.Enable=function(self) self.enabled=true end
                row.Show=function(self) self.visible=true end
                row.Hide=function(self) self.visible=false end
                return row
            end
            F.rows[1]=makeRow()
        """)
        lua.globals().GetLocale = lambda: locale
        entry = lua.table_from({"id": 2, "raceID": race, "policyRaceID": race, "name": "Synthetic", "faction": "Alliance"})
        lua.globals().F.entries[1] = entry
        if personal:
            lua.globals().F.draft.Synthetic = True
        start = self.generated.index("function CharacterCreate_RefreshRaceFilterRows()")
        end = self.generated.index("function CharacterCreate_ToggleRaceFilter()", start)
        lua.execute(self.prefix + "\nlocal filter=F; local VISIBLE=1; local UpdateScrollBar=function() end\n"
                    + self.generated[start:end])
        lua.globals().CharacterCreate_RefreshRaceFilterRows()
        return lua

    def test_entire_generated_source_compiles_in_lua51(self):
        self.runtime_type().execute("assert(loadstring(...))", self.generated)

    def test_disabled_row_unchecked_disabled_and_labelled(self):
        for race in [82, 83, 97]:
            with self.subTest(race=race):
                lua = self.rows(race)
                row = lua.globals().F.rows[1]
                self.assertFalse(row.checked)
                self.assertFalse(row.enabled)
                self.assertIn("Disabled by server", row.label.text)
                self.assertTrue(row.label.text.startswith("|cff808080"))

    def test_allowed_row_retains_personal_preference(self):
        for personal in [False, True]:
            with self.subTest(personal=personal):
                row = self.rows(13, personal).globals().F.rows[1]
                self.assertTrue(row.enabled)
                self.assertEqual(bool(row.checked), not personal)

    def test_recycled_scroll_row_is_reenabled(self):
        lua = self.rows()
        lua.globals().F.entries[1].policyRaceID = 13
        lua.globals().F.entries[1].raceID = 13
        lua.globals().CharacterCreate_RefreshRaceFilterRows()
        self.assertTrue(lua.globals().F.rows[1].enabled)
        self.assertTrue(lua.globals().F.rows[1].checked)

    def test_unknown_identity_never_treated_as_button_index(self):
        lua = self.rows()
        lua.globals().F.entries[1].policyRaceID = None
        lua.globals().F.entries[1].id = 82
        lua.globals().CharacterCreate_RefreshRaceFilterRows()
        self.assertTrue(lua.globals().F.rows[1].enabled)

    def test_check_all_cannot_remove_managed_restriction(self):
        lua = self.rows(82, True)
        start = self.generated.index("local function SetAllChecked(checked)")
        end = self.generated.index("local save = MakeGlueButton", start)
        check_all = lua.execute(self.prefix + "\nlocal f=F;\n" + self.generated[start:end] + "\nreturn SetAllChecked")
        check_all(True)
        self.assertFalse(lua.globals().F.rows[1].checked)
        self.assertFalse(lua.globals().F.rows[1].enabled)
        self.assertIsNone(lua.globals().F.draft.Synthetic)

    def test_disabled_click_callback_does_not_mutate_draft(self):
        lua = self.rows(82, True)
        start = self.generated.index("row:SetScript('OnClick', function(self)") + len("row:SetScript('OnClick', function(self)")
        end = self.generated.index("\n\t\t\tend)", start)
        callback = lua.execute("local f=F; return function(self)" + self.generated[start:end] + "\nend")
        row = lua.globals().F.rows[1]
        row.raceName = "Synthetic"
        callback(row)
        self.assertTrue(lua.globals().F.draft.Synthetic)

    def test_six_languages(self):
        for locale, label in [("enUS", "Disabled by server"), ("ruRU", "Отключено сервером"),
                              ("deDE", "Vom Server deaktiviert"), ("frFR", "Désactivé par le serveur"),
                              ("esES", "Desactivada por el servidor"), ("zhCN", "服务器已禁用")]:
            with self.subTest(locale=locale):
                self.assertIn(label, self.rows(locale=locale).globals().F.rows[1].label.text)

    def test_layout_fallback_never_reveals_managed_race(self):
        lua = self.runtime_type()
        lua.execute("""
            strlower=string.lower
            CharacterCreate_GetHiddenRaceSet=function() return {['#1']=true,['#2']=true} end
            GetFactionForRace=function() return '', 'Alliance' end
            function makeButton(i,race)
                local b={bonusRaceID=race,Text={GetText=function() return 'Synthetic' end}}
                b.Hide=function(self) self.visible=false end
                b.Show=function(self) self.visible=true end
                b.SetScale=function() end; b.ClearAllPoints=function() end
                b.SetPoint=function() end; b.SetFaction=function() end
                _G['CharCreateRaceButton'..i]=b
            end
            makeButton(1,1); makeButton(2,82)
        """)
        start = self.generated.index("function CharacterCreate_LayoutRaceButtons(")
        end = self.generated.index("MAX_CLASSES_PER_RACE =", start)
        lua.execute(self.prefix + self.generated[start:end])
        lua.globals().CharacterCreate_LayoutRaceButtons(2)
        self.assertTrue(lua.globals().CharCreateRaceButton1.visible)
        self.assertFalse(lua.globals().CharCreateRaceButton2.visible)

    def test_lifting_restriction_keeps_previous_personal_choice(self):
        for personal in [False, True]:
            with self.subTest(personal=personal):
                lua = self.rows(82, personal)
                generated = build_probe(self.source, []).decode()
                prefix = generated[:generated.index("CHAR_CREATE_HELP_NEXT_BUTTON")]
                start = generated.index("function CharacterCreate_RefreshRaceFilterRows()")
                end = generated.index("function CharacterCreate_ToggleRaceFilter()", start)
                lua.execute(prefix + "\nlocal filter=F; local VISIBLE=1; local UpdateScrollBar=function() end\n"
                            + generated[start:end])
                lua.globals().CharacterCreate_RefreshRaceFilterRows()
                row = lua.globals().F.rows[1]
                self.assertTrue(row.enabled)
                self.assertEqual(bool(row.checked), not personal)

    def test_unsupported_source_and_ids_fail(self):
        with self.assertRaises(ValueError):
            build_probe(b"unknown client source", [82])
        with self.assertRaises(ValueError):
            build_probe(self.source, [1])


if __name__ == "__main__":
    unittest.main()
