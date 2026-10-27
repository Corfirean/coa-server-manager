"""Synthetic MPQ roundtrips using an explicitly selected local Unicode StormLib DLL."""

import hashlib
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from race_availability_poc import Storm, generate_experiment, patch_dbc
from test_race_availability_poc import fixture


@unittest.skipUnless(os.environ.get("COA_TEST_STORMLIB"), "Set COA_TEST_STORMLIB to a Unicode StormLib DLL")
class MpqExperimentTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.storm = Storm(Path(os.environ["COA_TEST_STORMLIB"]))
        self.original = fixture(ids=(1, *range(12, 62)))
        source_dbc = self.root / "source.dbc"
        source_dbc.write_bytes(self.original)
        self.source_mpq = self.root / "source.MPQ"
        self.storm.create(self.source_mpq, source_dbc)
        self.source_bytes = self.source_mpq.read_bytes()
        self.output = self.root / "output"

    def test_one_and_fifty_races_still_one_archive_with_one_game_file(self):
        for count in [1, 50]:
            with self.subTest(count=count):
                output = self.root / f"output-{count}"
                disabled = list(range(12, 12 + count))
                report = generate_experiment(self.storm, self.source_mpq, output, disabled)
                archives = list(output.glob("*.MPQ"))
                self.assertEqual(len(archives), 1)
                header = struct.unpack_from("<4sIIHHIIII", archives[0].read_bytes())
                self.assertEqual(header[0], b"MPQ\x1a")
                self.assertEqual(header[-1], 2)
                listfile = self.root / f"listfile-{count}.txt"
                self.storm.extract(archives[0], "(listfile)", listfile)
                self.assertEqual(listfile.read_text().splitlines(), [r"DBFilesClient\ChrRaces.dbc"])
                self.assertEqual((output / "generated.dbc").read_bytes(), patch_dbc(self.original, disabled))
                self.assertEqual(report["mpqRoundtrip"], "PASS")
                self.assertEqual(report["creationMenu"], "NOT RUN")
                self.assertEqual(self.source_mpq.read_bytes(), self.source_bytes)

    def test_regeneration_has_identical_dbc_and_fingerprint(self):
        first = generate_experiment(self.storm, self.source_mpq, self.output, [12, 15])
        other = self.root / "other"
        second = generate_experiment(self.storm, self.source_mpq, other, [15, 12, 12])
        self.assertEqual(first["fingerprint"], second["fingerprint"])
        self.assertEqual((self.output / "generated.dbc").read_bytes(), (other / "generated.dbc").read_bytes())

    def test_existing_output_and_user_files_not_overwritten(self):
        self.output.mkdir()
        user_file = self.output / "experiment.MPQ"
        user_file.write_bytes(b"user archive")
        with self.assertRaises(FileExistsError):
            generate_experiment(self.storm, self.source_mpq, self.output, [12])
        self.assertEqual(user_file.read_bytes(), b"user archive")
        self.assertEqual(self.source_mpq.read_bytes(), self.source_bytes)

    def test_failed_packaging_does_not_publish_output(self):
        with patch.object(self.storm, "create", side_effect=OSError("synthetic packaging failure")):
            with self.assertRaises(OSError):
                generate_experiment(self.storm, self.source_mpq, self.output, [12])
        self.assertFalse(self.output.exists())
        self.assertEqual(self.source_mpq.read_bytes(), self.source_bytes)
        self.assertEqual(list(self.root.glob(".race-experiment-*")), [])

    def test_unreadable_archive_does_not_publish_output(self):
        broken = self.root / "broken.MPQ"
        broken.write_bytes(b"not an archive")
        with self.assertRaises(OSError):
            generate_experiment(self.storm, broken, self.output, [12])
        self.assertFalse(self.output.exists())
        self.assertEqual(broken.read_bytes(), b"not an archive")

    def test_invalid_race_does_not_publish_output(self):
        with self.assertRaises(ValueError):
            generate_experiment(self.storm, self.source_mpq, self.output, [255])
        self.assertFalse(self.output.exists())

    def test_empty_selection_preserves_original_bytes(self):
        report = generate_experiment(self.storm, self.source_mpq, self.output, [])
        self.assertEqual((self.output / "generated.dbc").read_bytes(), self.original)
        self.assertEqual(report["sourceMpqSha256"], hashlib.sha256(self.source_bytes).hexdigest())
        self.assertEqual(report["disabled"], [])

    def test_failed_roundtrip_does_not_publish_output(self):
        extract = self.storm.extract

        def corrupt_roundtrip(archive, member, destination):
            extract(archive, member, destination)
            if archive.name == "experiment.MPQ":
                destination.write_bytes(b"incorrect extracted bytes")

        with patch.object(self.storm, "extract", side_effect=corrupt_roundtrip):
            with self.assertRaisesRegex(ValueError, "MPQ extraction"):
                generate_experiment(self.storm, self.source_mpq, self.output, [12])
        self.assertFalse(self.output.exists())
        self.assertEqual(self.source_mpq.read_bytes(), self.source_bytes)


if __name__ == "__main__":
    unittest.main()
