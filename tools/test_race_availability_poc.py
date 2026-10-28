import struct
import unittest

from race_availability_poc import NAME_FIELDS, STOCK, fingerprint, patch_dbc, read_dbc


def fixture(ids=(1, 82, 83, 97), flags=0x8C):
    rows = bytearray()
    for race in ids:
        row = bytearray(276)
        struct.pack_into("<II", row, 0, race, flags)
        struct.pack_into("<I", row, 14 * 4, 1)
        rows.extend(row)
    strings = b"\0Synthetic race\0"
    return struct.pack("<4sIIII", b"WDBC", len(ids), 69, 276, len(strings)) + rows + strings


class DbcExperimentTests(unittest.TestCase):
    def test_only_requested_bit_changes(self):
        original = fixture()
        generated = patch_dbc(original, [82, 97])
        self.assertEqual([i for i, pair in enumerate(zip(original, generated)) if pair[0] != pair[1]],
                         [20 + 276 + 4, 20 + 3 * 276 + 4])
        self.assertEqual(read_dbc(generated)[82]["flags"], 0x8D)

    def test_empty_set_preserves_original(self):
        self.assertEqual(patch_dbc(fixture(), []), fixture())

    def test_duplicates_and_order_normalized(self):
        self.assertEqual(patch_dbc(fixture(), [97, 82, 82]), patch_dbc(fixture(), [82, 97]))

    def test_already_disabled_idempotent(self):
        original = fixture(flags=0x8D)
        self.assertEqual(patch_dbc(original, [82]), original)

    def test_each_generation_uses_original(self):
        original = fixture()
        patch_dbc(original, [82])
        self.assertEqual(read_dbc(patch_dbc(original, [97]))[82]["flags"], 0x8C)

    def test_unknown_or_stock_rejected(self):
        for race in [0, 42, 256, -1, *sorted(STOCK)]:
            with self.subTest(race=race), self.assertRaises(ValueError):
                patch_dbc(fixture(), [race])

    def test_bad_headers_rejected(self):
        for data in [b"", fixture()[:19], b"WDB2" + fixture()[4:], fixture(ids=()),
                     fixture()[:-1], fixture() + b"x", fixture(ids=(82, 82))]:
            with self.subTest(data=data[:20]), self.assertRaises(ValueError):
                read_dbc(data)

    def test_unrecognized_layout_rejected(self):
        for offset, value in [(8, 68), (12, 272), (16, 0), (4, 0xFFFFFFFF)]:
            data = bytearray(fixture())
            struct.pack_into("<I", data, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                read_dbc(data)

    def test_bad_string_offset_rejected(self):
        for field in NAME_FIELDS:
            data = bytearray(fixture())
            struct.pack_into("<I", data, 20 + field * 4, 99999)
            with self.subTest(field=field), self.assertRaises(ValueError):
                read_dbc(data)

    def test_nonempty_first_string_supported(self):
        data = bytearray(fixture())
        data[20 + 4 * 276] = ord("x")
        self.assertEqual(len(read_dbc(data)), 4)

    def test_reenable_restores_original_flags_including_original_bit(self):
        for flags in [0, 1, 12, 13, 0xFFFFFFFF]:
            original = fixture(flags=flags)
            patch_dbc(original, [82])
            self.assertEqual(patch_dbc(original, []), original)

    def test_one_five_and_fifty_races(self):
        original = fixture(ids=(1, *range(12, 62)))
        for count in [1, 5, 50]:
            disabled = set(range(12, 12 + count))
            generated = patch_dbc(original, disabled)
            original_rows, generated_rows = read_dbc(original), read_dbc(generated)
            self.assertEqual({race for race in original_rows
                              if original_rows[race]["flags"] != generated_rows[race]["flags"]}, disabled)

    def test_invalid_id_in_table(self):
        for race in [0, 256, 0xFFFFFFFF]:
            with self.subTest(race=race), self.assertRaises(ValueError):
                read_dbc(fixture(ids=(race,)))

    def test_string_block_must_terminate(self):
        with self.assertRaises(ValueError):
            read_dbc(fixture()[:-1] + b"x")

    def test_invalid_utf8_in_localized_name(self):
        data = bytearray(fixture())
        data[20 + 4 * 276 + 1] = 0xFF
        with self.assertRaises(ValueError):
            read_dbc(data)

    def test_fingerprint_normalizes_order_and_duplicates(self):
        self.assertEqual(fingerprint(fixture(), [97, 82, 82], "test"),
                         fingerprint(fixture(), [82, 97], "test"))

    def test_fingerprint_changes_with_every_input(self):
        baseline = fingerprint(fixture(), [82], "test")
        self.assertNotEqual(baseline, fingerprint(fixture(flags=0), [82], "test"))
        self.assertNotEqual(baseline, fingerprint(fixture(), [97], "test"))
        self.assertNotEqual(baseline, fingerprint(fixture(), [82], "other-profile"))


if __name__ == "__main__":
    unittest.main()
