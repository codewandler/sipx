#!/usr/bin/env python3
"""Contract tests for ``scripts/release-artifacts.py`` (A-10 and P-14)."""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import stat
import struct
import tempfile
import textwrap
import tomllib
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "release_artifacts", ROOT / "scripts" / "release-artifacts.py"
)
assert SPEC is not None and SPEC.loader is not None
artifacts = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(artifacts)

VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
    "package"
]["version"]
SHA = "a" * 40
EPOCH = 1_700_000_000


def elf64(*, interpreter: bool = False, needed: bool = False) -> bytes:
    """Return the smallest ELF shape the linkage parser needs."""

    program_count = int(interpreter or needed)
    size = 64 + 56 * program_count
    dynamic_size = 32 if needed else 0
    payload_size = 8 if interpreter else dynamic_size
    data = bytearray(size + payload_size)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<H", data, 52, 64)
    struct.pack_into("<H", data, 54, 56)
    struct.pack_into("<H", data, 56, program_count)
    if program_count:
        kind = 3 if interpreter else 2
        offset = 64 + 56
        struct.pack_into("<IIQQQQQQ", data, 64, kind, 0, offset, 0, 0, payload_size, payload_size, 1)
        if needed:
            struct.pack_into("<QQ", data, offset, 1, 0)
            struct.pack_into("<QQ", data, offset + 16, 0, 0)
    return bytes(data)


def metadata_fixture(version: str = VERSION) -> dict[str, object]:
    root = f"sipx-cli {version} (path+file:///sipx-cli)"
    dependency = "fixture 1.2.3 (registry+https://github.com/rust-lang/crates.io-index)"
    unused = "unused-native 9.9.9 (registry+https://github.com/rust-lang/crates.io-index)"
    return {
        "packages": [
            {
                "id": root,
                "name": "sipx-cli",
                "version": version,
                "source": None,
                "license": "MIT OR Apache-2.0",
                "features": {"native": ["dep:unused-native"]},
                "dependencies": [
                    {
                        "name": "fixture",
                        "rename": None,
                        "kind": None,
                        "target": None,
                        "optional": False,
                    },
                    {
                        "name": "unused-native",
                        "rename": None,
                        "kind": None,
                        "target": None,
                        "optional": True,
                    },
                ],
            },
            {
                "id": dependency,
                "name": "fixture",
                "version": "1.2.3",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "license": "MIT",
                "features": {},
                "dependencies": [],
            },
            {
                "id": unused,
                "name": "unused-native",
                "version": "9.9.9",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "license": "MIT",
                "features": {},
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {
                    "id": root,
                    "features": [],
                    "deps": [
                        {
                            "name": "fixture",
                            "pkg": dependency,
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                        {
                            "name": "unused_native",
                            "pkg": unused,
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {"id": dependency, "features": [], "deps": []},
                {"id": unused, "features": [], "deps": []},
            ]
        },
    }


def write_lock(path: pathlib.Path) -> None:
    path.write_text(
        textwrap.dedent(
            f"""\
            version = 4

            [[package]]
            name = "fixture"
            version = "1.2.3"
            source = "registry+https://github.com/rust-lang/crates.io-index"
            checksum = "{'b' * 64}"
            """
        ),
        encoding="utf-8",
    )


def sbom(target: str, directory: pathlib.Path) -> dict[str, object]:
    lock = directory / "Cargo.lock"
    write_lock(lock)
    return artifacts.spdx_document(
        metadata_fixture(),
        lock,
        version=VERSION,
        target=target,
        release_sha=SHA,
        epoch=EPOCH,
    )


def write_smoke_fixture(path: pathlib.Path, *, version: str = VERSION, hang: bool = False) -> None:
    delay = "time.sleep(30)" if hang else "time.sleep(0.1)"
    path.write_text(
        textwrap.dedent(
            f"""\
            #!/usr/bin/env python3
            import json
            import sys
            import time

            command = sys.argv[1]
            if command == "version":
                print("sipx {version}")
            elif command == "answer":
                {delay}
                print(json.dumps({{"status": "listening", "address": "127.0.0.1:9"}}), flush=True)
                if {hang!r}:
                    time.sleep(30)
                else:
                    print(json.dumps({{"status": "answered"}}), flush=True)
            elif command == "dial":
                print(json.dumps({{"status": "answered"}}))
            else:
                raise SystemExit(2)
            """
        ),
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class StaticLinkageIsObserved(unittest.TestCase):
    def test_a_static_elf_has_no_problems(self) -> None:
        self.assertEqual([], artifacts.elf_linkage_problems(elf64()))

    def test_an_interpreter_is_refused(self) -> None:
        self.assertTrue(
            any("PT_INTERP" in problem for problem in artifacts.elf_linkage_problems(elf64(interpreter=True)))
        )

    def test_a_shared_dependency_is_refused(self) -> None:
        self.assertTrue(
            any("DT_NEEDED" in problem for problem in artifacts.elf_linkage_problems(elf64(needed=True)))
        )

    def test_a_target_name_cannot_make_non_elf_bytes_static(self) -> None:
        self.assertTrue(artifacts.elf_linkage_problems(b"not an executable"))


class TheNativeSmokeProof(unittest.TestCase):
    def test_version_and_one_answered_call_pass(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / "sipx-fixture"
            write_smoke_fixture(binary)
            artifacts.smoke_binary(binary, VERSION, timeout=2)

    def test_a_wrong_version_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / "sipx-fixture"
            write_smoke_fixture(binary, version="9.9.9")
            with self.assertRaisesRegex(artifacts.ArtifactError, "version output differs"):
                artifacts.smoke_binary(binary, VERSION, timeout=2)

    def test_readiness_timeout_is_bounded_and_reaped(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / "sipx-fixture"
            write_smoke_fixture(binary, hang=True)
            with self.assertRaisesRegex(artifacts.ArtifactError, "readiness"):
                artifacts.smoke_binary(binary, VERSION, timeout=0.2)
            self.assertEqual({}, artifacts._OWNED)


class TheExactTargetSbom(unittest.TestCase):
    def test_only_the_selected_normal_closure_is_present(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            document = sbom("x86_64-apple-darwin", pathlib.Path(directory))
        names = {package["name"] for package in document["packages"]}
        self.assertEqual({"sipx-cli", "fixture"}, names)
        fixture = next(package for package in document["packages"] if package["name"] == "fixture")
        self.assertEqual("b" * 64, fixture["checksums"][0]["checksumValue"])

    def test_an_activated_optional_dependency_is_included(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            lock = root / "Cargo.lock"
            write_lock(lock)
            metadata = metadata_fixture()
            metadata["resolve"]["nodes"][0]["features"] = ["native"]
            document = artifacts.spdx_document(
                metadata,
                lock,
                version=VERSION,
                target="x86_64-apple-darwin",
                release_sha=SHA,
                epoch=EPOCH,
            )
        self.assertIn("unused-native", {package["name"] for package in document["packages"]})

    def test_creation_time_and_namespace_are_release_bound(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            document = sbom("aarch64-apple-darwin", pathlib.Path(directory))
        self.assertIn(SHA, document["documentNamespace"])
        self.assertEqual("2023-11-14T22:13:20Z", document["creationInfo"]["created"])
        self.assertEqual(
            [],
            artifacts.spdx_problems(
                document,
                version=VERSION,
                target="aarch64-apple-darwin",
                release_sha=SHA,
                epoch=EPOCH,
            ),
        )

    def test_an_edge_to_an_absent_package_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            document = sbom("x86_64-apple-darwin", pathlib.Path(directory))
        document["relationships"].append(
            {
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": "SPDXRef-Package-absent",
            }
        )
        problems = artifacts.spdx_problems(
            document,
            version=VERSION,
            target="x86_64-apple-darwin",
            release_sha=SHA,
            epoch=EPOCH,
        )
        self.assertTrue(any("absent" in problem for problem in problems), problems)


class PackagingAndAggregation(unittest.TestCase):
    def write_target(self, root: pathlib.Path, target: str) -> pathlib.Path:
        binary = root / ("fixture-" + target)
        binary.write_bytes(elf64() if artifacts.TARGETS[target][2] else b"native-fixture")
        manifest = artifacts._manifest(
            binary,
            version=VERSION,
            target=target,
            release_sha=SHA,
            epoch=EPOCH,
            rustc="rustc fixture",
            cargo="cargo fixture",
        )
        document = sbom(target, root)
        artifacts._write_assets(binary, manifest, document, root)
        return binary

    def write_matrix(self, root: pathlib.Path) -> None:
        for target in artifacts.TARGETS:
            self.write_target(root, target)
        for path in root.glob("fixture-*"):
            path.unlink()
        lock = root / "Cargo.lock"
        if lock.exists():
            lock.unlink()

    def test_archives_are_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            first_root = pathlib.Path(first)
            second_root = pathlib.Path(second)
            self.write_target(first_root, "aarch64-apple-darwin")
            self.write_target(second_root, "aarch64-apple-darwin")
            name = f"sipx-{VERSION}-aarch64-apple-darwin.tar.gz"
            self.assertEqual((first_root / name).read_bytes(), (second_root / name).read_bytes())

    def test_all_five_targets_aggregate_with_sorted_checksums(self) -> None:
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as output:
            source_root = pathlib.Path(source)
            output_root = pathlib.Path(output)
            self.write_matrix(source_root)
            paths = artifacts.aggregate(
                source_root,
                output_root,
                version=VERSION,
                release_sha=SHA,
                epoch=EPOCH,
            )
            self.assertEqual(11, len(paths))
            lines = (output_root / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
            self.assertEqual(10, len(lines))
            self.assertEqual(sorted(lines, key=lambda line: line.split("  ", 1)[1]), lines)

    def test_a_missing_target_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as output:
            source_root = pathlib.Path(source)
            self.write_matrix(source_root)
            missing = source_root / f"sipx-{VERSION}-aarch64-apple-darwin.spdx.json"
            missing.unlink()
            with self.assertRaisesRegex(artifacts.ArtifactError, "missing"):
                artifacts.aggregate(
                    source_root,
                    pathlib.Path(output),
                    version=VERSION,
                    release_sha=SHA,
                    epoch=EPOCH,
                )

    def test_a_manifest_not_bound_to_the_release_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as source, tempfile.TemporaryDirectory() as output:
            source_root = pathlib.Path(source)
            self.write_matrix(source_root)
            path = source_root / f"sipx-{VERSION}-x86_64-apple-darwin.build-manifest.json"
            manifest = json.loads(path.read_text(encoding="utf-8"))
            manifest["release_sha"] = "c" * 40
            path.write_bytes(artifacts._json_bytes(manifest))
            with self.assertRaisesRegex(artifacts.ArtifactError, "release_sha"):
                artifacts.aggregate(
                    source_root,
                    pathlib.Path(output),
                    version=VERSION,
                    release_sha=SHA,
                    epoch=EPOCH,
                )

    def test_existing_asset_bytes_must_match(self) -> None:
        with tempfile.TemporaryDirectory() as expected, tempfile.TemporaryDirectory() as actual:
            expected_path = pathlib.Path(expected) / "asset"
            actual_path = pathlib.Path(actual) / "asset"
            expected_path.write_bytes(b"expected")
            actual_path.write_bytes(b"changed")
            problems = artifacts.compare_assets(pathlib.Path(expected), pathlib.Path(actual))
            self.assertTrue(any("bytes differ" in problem for problem in problems), problems)

    def test_retry_allows_only_absent_assets(self) -> None:
        with tempfile.TemporaryDirectory() as expected, tempfile.TemporaryDirectory() as actual:
            expected_root = pathlib.Path(expected)
            actual_root = pathlib.Path(actual)
            (expected_root / "present").write_bytes(b"same")
            (expected_root / "later").write_bytes(b"later")
            (actual_root / "present").write_bytes(b"same")
            self.assertEqual(
                [], artifacts.compare_assets(expected_root, actual_root, allow_missing=True)
            )
            (actual_root / "unknown").write_bytes(b"unknown")
            problems = artifacts.compare_assets(expected_root, actual_root, allow_missing=True)
            self.assertTrue(any("unknown" in problem for problem in problems), problems)

    def test_package_runs_smoke_and_writes_three_assets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "sipx-fixture"
            write_smoke_fixture(binary)
            with mock.patch.object(artifacts, "_metadata", return_value=metadata_fixture()):
                paths = artifacts.package(
                    binary,
                    root / "out",
                    version=VERSION,
                    target="x86_64-apple-darwin",
                    release_sha=SHA,
                    epoch=EPOCH,
                    smoke_timeout=2,
                )
            self.assertEqual(3, len(paths))
            self.assertTrue(all(path.is_file() for path in paths))


if __name__ == "__main__":
    unittest.main(verbosity=2)
