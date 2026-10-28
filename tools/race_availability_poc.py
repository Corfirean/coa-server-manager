"""Offline experiment only; does not install patches or infer client load order."""

import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import struct
import tempfile


MEMBER = r"DBFilesClient\ChrRaces.dbc"
STOCK = {1, 2, 3, 4, 5, 6, 7, 8, 10, 11}
LAYOUT = (69, 276)
ALGORITHM_VERSION = 1
NAME_FIELDS = (*range(14, 30), *range(31, 47), *range(48, 64))


def read_dbc(data):
    if len(data) < 20:
        raise ValueError("Truncated DBC header")
    magic, count, fields, size, strings_size = struct.unpack_from("<4sIIII", data)
    if magic != b"WDBC" or (fields, size) != LAYOUT or not count:
        raise ValueError("Unsupported ChrRaces layout")
    strings_start = 20 + count * size
    if strings_start + strings_size != len(data) or not strings_size:
        raise ValueError("Invalid DBC bounds")
    strings = data[strings_start:]
    if strings[-1] != 0:
        raise ValueError("Invalid string block")
    rows = {}
    for index in range(count):
        offset = 20 + index * size
        race, flags = struct.unpack_from("<II", data, offset)
        if not 0 < race <= 255 or race in rows:
            raise ValueError("Invalid or duplicate RaceID")
        names = {}
        for field in NAME_FIELDS:
            name_offset = struct.unpack_from("<I", data, offset + field * 4)[0]
            if name_offset >= strings_size:
                raise ValueError("Invalid race name offset")
            names[field] = strings[name_offset:].split(b"\0", 1)[0].decode("utf-8", errors="strict")
        name = names[14]
        rows[race] = {"offset": offset, "flags": flags, "name": name}
    return rows


def patch_dbc(original, disabled):
    rows = read_dbc(original)
    disabled = set(disabled)
    if disabled & STOCK or not disabled <= rows.keys():
        raise ValueError("Only known custom RaceIDs can be disabled")
    result = bytearray(original)
    for race in disabled:
        row = rows[race]
        struct.pack_into("<I", result, row["offset"] + 4, row["flags"] | 1)
    verified = read_dbc(result)
    restored = bytearray(result)
    for race in disabled:
        row = rows[race]
        if verified[race]["flags"] != row["flags"] | 1:
            raise ValueError("Flags verification failed")
        struct.pack_into("<I", restored, row["offset"] + 4, row["flags"])
    if restored != original:
        raise ValueError("Unexpected byte changes")
    return bytes(result)


def fingerprint(original, disabled, profile):
    inputs = {"sourceDbcSha256": hashlib.sha256(original).hexdigest(),
              "disabledRaceIds": sorted(set(disabled)), "algorithmVersion": ALGORITHM_VERSION,
              "profile": profile}
    return hashlib.sha256(json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


class Storm:
    def __init__(self, library):
        self.api = ctypes.WinDLL(str(library), use_last_error=True)
        handle = ctypes.c_void_p
        word = ctypes.c_uint32
        signatures = {
            "SFileOpenArchive": [ctypes.c_wchar_p, word, word, ctypes.POINTER(handle)],
            "SFileCloseArchive": [handle],
            "SFileExtractFile": [handle, ctypes.c_char_p, ctypes.c_wchar_p, word],
            "SFileCreateArchive": [ctypes.c_wchar_p, word, word, ctypes.POINTER(handle)],
            "SFileAddFileEx": [handle, ctypes.c_wchar_p, ctypes.c_char_p, word, word, word],
        }
        for name, arguments in signatures.items():
            function = getattr(self.api, name)
            function.argtypes = arguments
            function.restype = ctypes.c_int

    def check(self, name, *args):
        if not getattr(self.api, name)(*args):
            raise OSError(ctypes.get_last_error(), name)

    def extract(self, archive, member, destination):
        handle = ctypes.c_void_p()
        self.check("SFileOpenArchive", str(archive), 0, 0x100, ctypes.byref(handle))
        try:
            self.check("SFileExtractFile", handle, member.encode("ascii"),
                       str(destination), 0)
        finally:
            self.check("SFileCloseArchive", handle)

    def create(self, archive, source):
        handle = ctypes.c_void_p()
        self.check("SFileCreateArchive", str(archive), 0, 4, ctypes.byref(handle))
        try:
            self.check("SFileAddFileEx", handle, str(source), MEMBER.encode("ascii"),
                       0x01000000, 0, 0)
        finally:
            self.check("SFileCloseArchive", handle)


def generate_experiment(storm, source_mpq, output, disabled, profile="unverified"):
    source_mpq = Path(source_mpq).resolve(strict=True)
    output = Path(output).absolute()
    if output.exists():
        raise FileExistsError(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".race-experiment-", dir=output.parent) as temp:
        staging = Path(temp) / "result"
        staging.mkdir()
        report = build_experiment(storm, source_mpq, staging, disabled, profile)
        staging.rename(output)
    return report


def build_experiment(storm, source_mpq, output, disabled, profile):
    with source_mpq.open("rb") as source:
        source_sha = hashlib.file_digest(source, "sha256").hexdigest()
    original_path = output / "original.dbc"
    storm.extract(source_mpq, MEMBER, original_path)
    original = original_path.read_bytes()
    rows = read_dbc(original)
    generated = patch_dbc(original, disabled)
    generated_path = output / "generated.dbc"
    generated_path.write_bytes(generated)
    overlay = output / "experiment.MPQ"
    storm.create(overlay, generated_path)
    with tempfile.TemporaryDirectory() as temp:
        extracted = Path(temp) / "roundtrip.dbc"
        storm.extract(overlay, MEMBER, extracted)
        if extracted.read_bytes() != generated:
            raise ValueError("MPQ extraction does not match generated DBC")
    report = {
        "sourceMpq": str(source_mpq), "sourceMpqSha256": source_sha,
        "algorithmVersion": ALGORITHM_VERSION, "profile": profile,
        "fingerprint": fingerprint(original, disabled, profile),
        "sourceDbcSha256": hashlib.sha256(original).hexdigest(),
        "generatedDbcSha256": hashlib.sha256(generated).hexdigest(),
        "disabled": [{"id": race, "name": rows[race]["name"],
                      "oldFlags": rows[race]["flags"], "newFlags": rows[race]["flags"] | 1}
                     for race in sorted(set(disabled))],
        "catalog": [{"id": race, "name": row["name"], "flags": row["flags"]}
                    for race, row in sorted(rows.items())],
        "dbcValidation": "PASS", "onlyRequestedBitChanged": "PASS", "mpqRoundtrip": "PASS",
        "effectiveSourceInRunningClient": "UNVERIFIED", "overlayPriority": "UNVERIFIED",
        "creationMenu": "NOT RUN", "otherRaceCreation": "NOT RUN",
        "existingCharacterLogin": "NOT RUN", "rollbackInGame": "NOT RUN",
        "verdict": "PARTIAL",
    }
    with source_mpq.open("rb") as source:
        if hashlib.file_digest(source, "sha256").hexdigest() != source_sha:
            raise ValueError("Source archive changed during the experiment")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--storm", type=Path, required=True)
    parser.add_argument("--source-mpq", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--disable", type=int, nargs="*", required=True)
    parser.add_argument("--profile", default="unverified", help="Recorded input; not a support declaration")
    args = parser.parse_args()
    report = generate_experiment(Storm(args.storm), args.source_mpq, args.output, args.disable, args.profile)
    print(json.dumps({key: value for key, value in report.items() if key != "catalog"}, indent=2))


if __name__ == "__main__":
    main()
