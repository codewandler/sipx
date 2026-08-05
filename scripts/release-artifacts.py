#!/usr/bin/env python3
"""Build and validate the portable CLI artifacts for one immutable release.

The contract is ``docs/specs/release-artifacts.md``.  This program owns the parts worth testing
locally: deterministic archives, exact-target SPDX data, static-ELF inspection, bounded native
smoke supervision and exact-set aggregation.  The workflow supplies native runners and release
authority; this program has no upload path.
"""

from __future__ import annotations

import argparse
import datetime
import gzip
import hashlib
import io
import json
import os
import pathlib
import queue
import re
import shutil
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import tomllib
import zipfile
from collections.abc import Iterable, Mapping, Sequence

ROOT = pathlib.Path(__file__).resolve().parent.parent
PACKAGE = "sipx-cli"
SCHEMA = "sipx.release-artifact.v1"
SPDX_VERSION = "SPDX-2.3"
MAX_OUTPUT = 16 * 1024 * 1024
MAX_READINESS = 65_536
TARGETS: Mapping[str, tuple[str, str, bool]] = {
    "x86_64-unknown-linux-musl": ("sipx", ".tar.gz", True),
    "aarch64-unknown-linux-musl": ("sipx", ".tar.gz", True),
    "x86_64-apple-darwin": ("sipx", ".tar.gz", False),
    "aarch64-apple-darwin": ("sipx", ".tar.gz", False),
    "x86_64-pc-windows-msvc": ("sipx.exe", ".zip", False),
}
MANIFEST_KEYS = {
    "schema",
    "version",
    "target",
    "release_sha",
    "source_date_epoch",
    "rustc",
    "cargo",
    "features",
    "binary",
    "binary_sha256",
    "static_linked",
}
_SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")
_SHA = re.compile(r"^[0-9a-f]{40}$")
_OWNED: dict[int, subprocess.Popen[bytes]] = {}


class ArtifactError(ValueError):
    """The requested bytes cannot satisfy the release-artifact contract."""


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode()


def _validate_inputs(version: str, target: str, release_sha: str, epoch: int) -> None:
    problems = []
    if not _SEMVER.fullmatch(version):
        problems.append(f"version is not semver: {version!r}")
    if target not in TARGETS:
        problems.append(f"unsupported artifact target: {target!r}")
    if not _SHA.fullmatch(release_sha):
        problems.append("release SHA must be forty lowercase hexadecimal digits")
    if epoch <= 0:
        problems.append("SOURCE_DATE_EPOCH must be positive")
    if problems:
        raise ArtifactError("\n".join(problems))


def elf_linkage_problems(data: bytes) -> list[str]:
    """Return dynamic-linkage defects from one ELF image without trusting its filename."""

    if len(data) < 52 or data[:4] != b"\x7fELF":
        return ["Linux musl artifact is not an ELF executable"]
    elf_class = data[4]
    byte_order = data[5]
    if elf_class not in (1, 2) or byte_order not in (1, 2):
        return ["ELF class or byte order is unsupported"]
    endian = "<" if byte_order == 1 else ">"
    if elf_class == 1:
        header_size = 52
        phoff = struct.unpack_from(endian + "I", data, 28)[0]
        phentsize = struct.unpack_from(endian + "H", data, 42)[0]
        phnum = struct.unpack_from(endian + "H", data, 44)[0]
        expected_ph = 32
        dynamic_entry = endian + "II"
    else:
        header_size = 64
        phoff = struct.unpack_from(endian + "Q", data, 32)[0]
        phentsize = struct.unpack_from(endian + "H", data, 54)[0]
        phnum = struct.unpack_from(endian + "H", data, 56)[0]
        expected_ph = 56
        dynamic_entry = endian + "QQ"
    if len(data) < header_size or (phnum and phentsize < expected_ph):
        return ["ELF program-header table is malformed"]
    if phoff + phentsize * phnum > len(data):
        return ["ELF program-header table leaves the executable"]

    problems = []
    dynamic_ranges: list[tuple[int, int]] = []
    for index in range(phnum):
        offset = phoff + index * phentsize
        kind = struct.unpack_from(endian + "I", data, offset)[0]
        if elf_class == 1:
            file_offset = struct.unpack_from(endian + "I", data, offset + 4)[0]
            file_size = struct.unpack_from(endian + "I", data, offset + 16)[0]
        else:
            file_offset = struct.unpack_from(endian + "Q", data, offset + 8)[0]
            file_size = struct.unpack_from(endian + "Q", data, offset + 32)[0]
        if kind == 3:
            problems.append("ELF carries a PT_INTERP dynamic loader")
        if kind == 2:
            dynamic_ranges.append((file_offset, file_size))

    entry_size = struct.calcsize(dynamic_entry)
    for file_offset, file_size in dynamic_ranges:
        if file_offset + file_size > len(data) or file_size % entry_size:
            problems.append("ELF dynamic table is malformed")
            continue
        for offset in range(file_offset, file_offset + file_size, entry_size):
            tag, _value = struct.unpack_from(dynamic_entry, data, offset)
            if tag == 0:
                break
            if tag == 1:
                problems.append("ELF carries a DT_NEEDED shared-library dependency")
                break
    return problems


def _process_options() -> dict[str, object]:
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def _terminate_tree(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        try:
            subprocess.run(
                ("taskkill", "/PID", str(process.pid), "/T", "/F"),
                capture_output=True,
                timeout=5,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            process.kill()
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=2)
            return
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def _close_process_pipes(process: subprocess.Popen[bytes]) -> None:
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            stream.close()


def _bounded_run(command: Sequence[str], timeout: float) -> subprocess.CompletedProcess[bytes]:
    process = subprocess.Popen(
        tuple(command),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **_process_options(),
    )
    _OWNED[process.pid] = process
    try:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            _terminate_tree(process)
            stdout, stderr = process.communicate()
            raise ArtifactError(
                f"command exceeded its {timeout:g}s bound: {' '.join(command)}"
            ) from error
        if len(stdout) > MAX_OUTPUT or len(stderr) > MAX_OUTPUT:
            raise ArtifactError(f"command output exceeded {MAX_OUTPUT} bytes: {' '.join(command)}")
        return subprocess.CompletedProcess(tuple(command), process.returncode, stdout, stderr)
    finally:
        owned = _OWNED.pop(process.pid, None)
        if owned is not None:
            _terminate_tree(owned)
            _close_process_pipes(owned)


def _readline(process: subprocess.Popen[bytes], timeout: float) -> bytes:
    if process.stdout is None:
        raise ArtifactError("answerer has no readiness pipe")
    outcome: queue.Queue[bytes | BaseException] = queue.Queue(maxsize=1)

    def read() -> None:
        line = bytearray()
        try:
            while len(line) <= MAX_READINESS:
                # Do not use BufferedReader.read here.  It may pull the event after the
                # readiness line into its private buffer; communicate() consumes the raw
                # descriptor later and would then lose that event.
                byte = os.read(process.stdout.fileno(), 1)
                if not byte or byte == b"\n":
                    break
                line.extend(byte)
            if len(line) > MAX_READINESS:
                raise ArtifactError(f"answerer readiness exceeds {MAX_READINESS} bytes")
            outcome.put(bytes(line))
        except BaseException as error:  # the parent re-raises after bounded cleanup
            outcome.put(error)

    worker = threading.Thread(target=read, name="release-artifact-readiness", daemon=True)
    worker.start()
    try:
        value = outcome.get(timeout=timeout)
    except queue.Empty as error:
        raise ArtifactError(f"answerer did not report readiness within {timeout:g}s") from error
    if isinstance(value, BaseException):
        raise ArtifactError(f"cannot read answerer readiness: {value}") from value
    if not value:
        raise ArtifactError("answerer closed before its readiness report")
    return value


def smoke_binary(binary: pathlib.Path, version: str, timeout: float = 30.0) -> None:
    """Prove the native executable's version and one complete bounded UDP call."""

    version_result = _bounded_run((str(binary), "version"), min(timeout, 10.0))
    expected = f"sipx {version}\n".encode()
    if version_result.returncode != 0 or version_result.stdout != expected:
        raise ArtifactError(
            f"artifact version output differs: status {version_result.returncode}, "
            f"stdout {version_result.stdout!r}"
        )

    answerer = subprocess.Popen(
        (
            str(binary),
            "answer",
            "--local",
            "127.0.0.1:0",
            "--json",
            "--wait",
            "10",
            "--duration",
            "1",
            "--once",
        ),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **_process_options(),
    )
    _OWNED[answerer.pid] = answerer
    try:
        line = _readline(answerer, min(timeout, 15.0))
        try:
            readiness = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ArtifactError(f"answerer readiness is not JSON: {line!r}") from error
        address = readiness.get("address")
        if readiness.get("status") != "listening" or not isinstance(address, str):
            raise ArtifactError(f"answerer readiness is not listening: {readiness!r}")

        dial = _bounded_run(
            (
                str(binary),
                "dial",
                f"sip:release-proof@{address}",
                "--json",
                "--timeout",
                "10",
                "--duration",
                "1",
            ),
            timeout,
        )
        if dial.returncode != 0:
            raise ArtifactError(
                f"artifact dial failed with {dial.returncode}: "
                + dial.stderr.decode(errors="replace").strip()
            )
        try:
            dial_reports = [json.loads(line) for line in dial.stdout.splitlines() if line]
        except json.JSONDecodeError as error:
            raise ArtifactError(f"artifact dial output is not JSON: {dial.stdout!r}") from error
        if not any(report.get("status") == "answered" for report in dial_reports):
            raise ArtifactError(f"artifact dial did not answer: {dial_reports!r}")

        try:
            stdout, stderr = answerer.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise ArtifactError(f"artifact answerer exceeded its {timeout:g}s bound") from error
        if len(stdout) > MAX_OUTPUT or len(stderr) > MAX_OUTPUT:
            raise ArtifactError("artifact answerer output exceeded its bound")
        if answerer.returncode != 0:
            raise ArtifactError(
                f"artifact answerer failed with {answerer.returncode}: "
                + stderr.decode(errors="replace").strip()
            )
        try:
            answer_reports = [json.loads(line) for line in stdout.splitlines() if line]
        except json.JSONDecodeError as error:
            raise ArtifactError(f"artifact answer output is not JSON: {stdout!r}") from error
        if not any(report.get("status") == "answered" for report in answer_reports):
            raise ArtifactError(f"artifact answerer emitted no answered report: {answer_reports!r}")
    finally:
        owned = _OWNED.pop(answerer.pid, None)
        if owned is not None:
            _terminate_tree(owned)
            _close_process_pipes(owned)


def _metadata(target: str) -> dict[str, object]:
    command = (
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        target,
        "--no-default-features",
    )
    result = _bounded_run(command, 120.0)
    if result.returncode != 0:
        raise ArtifactError("cargo metadata failed: " + result.stderr.decode(errors="replace"))
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ArtifactError("cargo metadata emitted invalid JSON") from error
    if not isinstance(value, dict):
        raise ArtifactError("cargo metadata did not emit an object")
    return value


def _lock_checksums(lock_path: pathlib.Path) -> dict[tuple[str, str, str], str]:
    lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    checksums = {}
    for package in lock.get("package", []):
        if not isinstance(package, dict):
            continue
        name = package.get("name")
        version = package.get("version")
        source = package.get("source")
        checksum = package.get("checksum")
        if all(isinstance(value, str) for value in (name, version, source, checksum)):
            checksums[(name, version, source)] = checksum
    return checksums


def _dependency_key(name: str) -> str:
    return name.replace("-", "_")


def _active_optional_dependencies(
    package: Mapping[str, object], node: Mapping[str, object]
) -> set[str]:
    definitions = package.get("features")
    active = node.get("features")
    if not isinstance(definitions, dict) or not isinstance(active, list):
        raise ArtifactError("Cargo metadata omits package feature resolution")
    pending = [feature for feature in active if isinstance(feature, str)]
    visited = set()
    dependencies = set()
    while pending:
        feature = pending.pop()
        if feature in visited:
            continue
        visited.add(feature)
        rules = definitions.get(feature, [])
        if not isinstance(rules, list):
            raise ArtifactError(f"Cargo metadata feature {feature!r} is not a list")
        for rule in rules:
            if not isinstance(rule, str):
                raise ArtifactError(f"Cargo metadata feature {feature!r} has a non-string rule")
            if rule.startswith("dep:"):
                dependencies.add(_dependency_key(rule[4:]))
                continue
            dependency, separator, _dependency_feature = rule.partition("/")
            if separator:
                if not dependency.endswith("?"):
                    dependencies.add(_dependency_key(dependency))
                continue
            if rule in definitions:
                pending.append(rule)
            else:
                # Cargo's older implicit optional-feature spelling uses the dependency name.
                dependencies.add(_dependency_key(rule))
    return dependencies


def _normal_edges(
    package: Mapping[str, object],
    node: Mapping[str, object],
    packages: Mapping[str, Mapping[str, object]],
) -> tuple[str, ...]:
    declarations = package.get("dependencies")
    if not isinstance(declarations, list):
        raise ArtifactError("Cargo metadata omits package dependency declarations")
    active_optional = _active_optional_dependencies(package, node)
    edges = []
    for dependency in node.get("deps", []):
        if (
            not isinstance(dependency, dict)
            or not isinstance(dependency.get("pkg"), str)
            or not isinstance(dependency.get("name"), str)
        ):
            continue
        kinds = dependency.get("dep_kinds", [])
        if not isinstance(kinds, list):
            continue
        normal_kinds = {
            kind.get("target")
            for kind in kinds
            if isinstance(kind, dict) and kind.get("kind") is None
        }
        if not normal_kinds:
            continue
        target_package = packages.get(str(dependency["pkg"]))
        if target_package is None or not isinstance(target_package.get("name"), str):
            raise ArtifactError(f"resolved dependency has no package: {dependency['pkg']}")
        resolved_name = _dependency_key(str(dependency["name"]))
        matching = [
            declaration
            for declaration in declarations
            if isinstance(declaration, dict)
            and declaration.get("kind") is None
            and declaration.get("target") in normal_kinds
            and declaration.get("name") == target_package["name"]
            and (
                declaration.get("rename") is None
                or _dependency_key(str(declaration["rename"])) == resolved_name
            )
        ]
        if not matching:
            raise ArtifactError(
                f"resolved dependency has no normal declaration: {resolved_name}"
            )
        if any(not declaration.get("optional", False) for declaration in matching) or (
            any(
                _dependency_key(
                    str(declaration.get("rename") or declaration.get("name") or "")
                )
                in active_optional
                for declaration in matching
            )
        ):
            edges.append(str(dependency["pkg"]))
    return tuple(sorted(set(edges)))


def _spdx_id(package_id: str) -> str:
    return "SPDXRef-Package-" + hashlib.sha256(package_id.encode()).hexdigest()[:20]


def spdx_document(
    metadata: Mapping[str, object],
    lock_path: pathlib.Path,
    *,
    version: str,
    target: str,
    release_sha: str,
    epoch: int,
) -> dict[str, object]:
    """Derive one deterministic SPDX 2.3 document from the selected normal closure."""

    _validate_inputs(version, target, release_sha, epoch)
    packages_raw = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages_raw, list) or not isinstance(resolve, dict):
        raise ArtifactError("Cargo metadata omits packages or the resolve graph")
    packages = {
        str(package["id"]): package
        for package in packages_raw
        if isinstance(package, dict) and isinstance(package.get("id"), str)
    }
    roots = [
        package_id
        for package_id, package in packages.items()
        if package.get("name") == PACKAGE and package.get("version") == version
    ]
    if len(roots) != 1:
        raise ArtifactError(f"metadata must contain one {PACKAGE} {version} root, found {len(roots)}")
    root = roots[0]
    nodes_raw = resolve.get("nodes")
    if not isinstance(nodes_raw, list):
        raise ArtifactError("Cargo metadata resolve graph omits nodes")
    nodes = {
        str(node["id"]): node
        for node in nodes_raw
        if isinstance(node, dict) and isinstance(node.get("id"), str)
    }
    if root not in nodes:
        raise ArtifactError(f"resolve graph omits {PACKAGE}")

    closure = set()
    pending = [root]
    while pending:
        package_id = pending.pop()
        if package_id in closure:
            continue
        if package_id not in packages or package_id not in nodes:
            raise ArtifactError(f"normal dependency is absent from metadata: {package_id}")
        closure.add(package_id)
        pending.extend(
            edge
            for edge in _normal_edges(packages[package_id], nodes[package_id], packages)
            if edge not in closure
        )

    checksums = _lock_checksums(lock_path)
    spdx_packages = []
    for package_id in sorted(closure):
        package = packages[package_id]
        name = package.get("name")
        package_version = package.get("version")
        source = package.get("source")
        if not isinstance(name, str) or not isinstance(package_version, str):
            raise ArtifactError(f"package has no name/version identity: {package_id}")
        source_text = source if isinstance(source, str) else ""
        licence = package.get("license")
        declared = licence if isinstance(licence, str) and licence else "NOASSERTION"
        record: dict[str, object] = {
            "SPDXID": _spdx_id(package_id),
            "name": name,
            "versionInfo": package_version,
            "downloadLocation": (
                f"https://crates.io/crates/{name}/{package_version}"
                if source_text.startswith("registry+")
                else f"https://github.com/codewandler/sipx/tree/{release_sha}"
            ),
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": declared,
            "copyrightText": "NOASSERTION",
            "externalRefs": [
                {
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": f"pkg:cargo/{name}@{package_version}",
                }
            ],
        }
        checksum = checksums.get((name, package_version, source_text))
        if checksum is not None:
            record["checksums"] = [{"algorithm": "SHA256", "checksumValue": checksum}]
        spdx_packages.append(record)

    relationships: list[dict[str, str]] = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": _spdx_id(root),
        }
    ]
    for package_id in sorted(closure):
        for dependency in _normal_edges(
            packages[package_id], nodes[package_id], packages
        ):
            if dependency not in closure:
                raise ArtifactError(f"SPDX edge leaves the selected closure: {dependency}")
            relationships.append(
                {
                    "spdxElementId": _spdx_id(package_id),
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": _spdx_id(dependency),
                }
            )
    created = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    return {
        "spdxVersion": SPDX_VERSION,
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"sipx-{version}-{target}",
        "documentNamespace": (
            "https://github.com/codewandler/sipx/releases/download/"
            f"v{version}/sipx-{version}-{target}.spdx.json?sha={release_sha}"
        ),
        "creationInfo": {"created": created, "creators": ["Tool: sipx-release-artifacts"]},
        "packages": sorted(spdx_packages, key=lambda item: str(item["SPDXID"])),
        "relationships": sorted(
            relationships,
            key=lambda item: (
                item["spdxElementId"], item["relationshipType"], item["relatedSpdxElement"]
            ),
        ),
    }


def spdx_problems(
    document: Mapping[str, object], *, version: str, target: str, release_sha: str, epoch: int
) -> list[str]:
    problems = []
    if document.get("spdxVersion") != SPDX_VERSION:
        problems.append(f"SBOM does not carry {SPDX_VERSION}")
    if document.get("name") != f"sipx-{version}-{target}":
        problems.append("SBOM name differs from the release target")
    if release_sha not in str(document.get("documentNamespace", "")):
        problems.append("SBOM namespace is not bound to the release commit")
    created = datetime.datetime.fromtimestamp(epoch, datetime.timezone.utc).strftime(
        "%Y-%m-%dT%H:%M:%SZ"
    )
    creation = document.get("creationInfo")
    if not isinstance(creation, dict) or creation.get("created") != created:
        problems.append("SBOM creation time differs from SOURCE_DATE_EPOCH")
    packages = document.get("packages")
    if not isinstance(packages, list) or not packages:
        return problems + ["SBOM contains no packages"]
    identifiers = [
        package.get("SPDXID") for package in packages if isinstance(package, dict)
    ]
    if len(identifiers) != len(packages) or any(not isinstance(value, str) for value in identifiers):
        problems.append("SBOM package lacks an SPDX identifier")
    if len(set(identifiers)) != len(identifiers):
        problems.append("SBOM package identifiers are not unique")
    known = {"SPDXRef-DOCUMENT", *identifiers}
    relationships = document.get("relationships")
    if not isinstance(relationships, list):
        problems.append("SBOM relationships are absent")
        return problems
    describes = 0
    for relationship in relationships:
        if not isinstance(relationship, dict):
            problems.append("SBOM relationship is not an object")
            continue
        left = relationship.get("spdxElementId")
        right = relationship.get("relatedSpdxElement")
        if left not in known or right not in known:
            problems.append("SBOM relationship names an absent element")
        if left == "SPDXRef-DOCUMENT" and relationship.get("relationshipType") == "DESCRIBES":
            describes += 1
    if describes != 1:
        problems.append("SBOM must describe exactly one executable package")
    return problems


def _manifest(
    binary: pathlib.Path,
    *,
    version: str,
    target: str,
    release_sha: str,
    epoch: int,
    rustc: str,
    cargo: str,
) -> dict[str, object]:
    binary_name, _suffix, static = TARGETS[target]
    return {
        "schema": SCHEMA,
        "version": version,
        "target": target,
        "release_sha": release_sha,
        "source_date_epoch": epoch,
        "rustc": rustc.strip(),
        "cargo": cargo.strip(),
        "features": [],
        "binary": binary_name,
        "binary_sha256": _sha256(binary.read_bytes()),
        "static_linked": static,
    }


def manifest_problems(
    manifest: Mapping[str, object], *, version: str, target: str, release_sha: str, epoch: int
) -> list[str]:
    problems = []
    keys = set(manifest)
    if keys != MANIFEST_KEYS:
        problems.append(
            "build manifest keys differ: "
            f"missing {sorted(MANIFEST_KEYS - keys)}, unknown {sorted(keys - MANIFEST_KEYS)}"
        )
    expected = {
        "schema": SCHEMA,
        "version": version,
        "target": target,
        "release_sha": release_sha,
        "source_date_epoch": epoch,
        "features": [],
        "binary": TARGETS[target][0],
        "static_linked": TARGETS[target][2],
    }
    for key, value in expected.items():
        if manifest.get(key) != value:
            problems.append(f"build manifest {key} differs: {manifest.get(key)!r} != {value!r}")
    if not isinstance(manifest.get("binary_sha256"), str) or not re.fullmatch(
        r"[0-9a-f]{64}", str(manifest.get("binary_sha256", ""))
    ):
        problems.append("build manifest binary_sha256 is malformed")
    for key in ("rustc", "cargo"):
        if not isinstance(manifest.get(key), str) or not str(manifest.get(key)).strip():
            problems.append(f"build manifest {key} is empty")
    return problems


def _tar_bytes(entries: Mapping[str, tuple[bytes, int]], epoch: int) -> bytes:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name in sorted(entries):
            data, mode = entries[name]
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = mode
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = epoch
            archive.addfile(info, io.BytesIO(data))
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode="wb", filename="", mtime=epoch) as output:
        output.write(raw.getvalue())
    return compressed.getvalue()


def _zip_bytes(entries: Mapping[str, tuple[bytes, int]], epoch: int) -> bytes:
    output = io.BytesIO()
    stamp = datetime.datetime.fromtimestamp(max(epoch, 315532800), datetime.timezone.utc)
    date_time = (stamp.year, stamp.month, stamp.day, stamp.hour, stamp.minute, stamp.second)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name in sorted(entries):
            data, mode = entries[name]
            info = zipfile.ZipInfo(name, date_time=date_time)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = mode << 16
            archive.writestr(info, data)
    return output.getvalue()


def _write_assets(
    binary: pathlib.Path,
    manifest: Mapping[str, object],
    sbom: Mapping[str, object],
    out: pathlib.Path,
) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path]:
    version = str(manifest["version"])
    target = str(manifest["target"])
    epoch = int(manifest["source_date_epoch"])
    binary_name, suffix, _static = TARGETS[target]
    base = f"sipx-{version}-{target}"
    manifest_bytes = _json_bytes(manifest)
    entries = {
        f"{base}/{binary_name}": (binary.read_bytes(), 0o755),
        f"{base}/build-manifest.json": (manifest_bytes, 0o644),
        f"{base}/LICENSE-APACHE": ((ROOT / "LICENSE-APACHE").read_bytes(), 0o644),
        f"{base}/LICENSE-MIT": ((ROOT / "LICENSE-MIT").read_bytes(), 0o644),
    }
    out.mkdir(parents=True, exist_ok=True)
    archive_path = out / f"{base}{suffix}"
    archive_path.write_bytes(
        _zip_bytes(entries, epoch) if suffix == ".zip" else _tar_bytes(entries, epoch)
    )
    manifest_path = out / f"{base}.build-manifest.json"
    manifest_path.write_bytes(manifest_bytes)
    sbom_path = out / f"{base}.spdx.json"
    sbom_path.write_bytes(_json_bytes(sbom))
    return archive_path, manifest_path, sbom_path


def package(
    binary: pathlib.Path,
    out: pathlib.Path,
    *,
    version: str,
    target: str,
    release_sha: str,
    epoch: int,
    smoke_timeout: float,
) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path]:
    _validate_inputs(version, target, release_sha, epoch)
    if not binary.is_file():
        raise ArtifactError(f"release binary does not exist: {binary}")
    if TARGETS[target][2]:
        problems = elf_linkage_problems(binary.read_bytes())
        if problems:
            raise ArtifactError("\n".join(problems))
    smoke_binary(binary, version, smoke_timeout)
    rustc = _bounded_run(("rustc", "--version"), 10.0)
    cargo = _bounded_run(("cargo", "--version"), 10.0)
    if rustc.returncode or cargo.returncode:
        raise ArtifactError("cannot identify the release Rust toolchain")
    manifest = _manifest(
        binary,
        version=version,
        target=target,
        release_sha=release_sha,
        epoch=epoch,
        rustc=rustc.stdout.decode(errors="replace"),
        cargo=cargo.stdout.decode(errors="replace"),
    )
    metadata = _metadata(target)
    sbom = spdx_document(
        metadata,
        ROOT / "Cargo.lock",
        version=version,
        target=target,
        release_sha=release_sha,
        epoch=epoch,
    )
    return _write_assets(binary, manifest, sbom, out)


def _archive_entries(path: pathlib.Path, target: str, version: str) -> dict[str, bytes]:
    base = f"sipx-{version}-{target}"
    expected = {
        f"{base}/{TARGETS[target][0]}",
        f"{base}/build-manifest.json",
        f"{base}/LICENSE-APACHE",
        f"{base}/LICENSE-MIT",
    }
    found: dict[str, bytes] = {}
    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())
            if names != expected:
                raise ArtifactError(
                    f"{path.name}: archive entries differ: missing {sorted(expected - names)}, "
                    f"unknown {sorted(names - expected)}"
                )
            for info in archive.infolist():
                if info.is_dir() or (info.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ArtifactError(f"{path.name}: non-regular archive entry {info.filename}")
                found[info.filename] = archive.read(info)
    else:
        with tarfile.open(path, "r:gz") as archive:
            members = archive.getmembers()
            names = {member.name for member in members}
            if names != expected:
                raise ArtifactError(
                    f"{path.name}: archive entries differ: missing {sorted(expected - names)}, "
                    f"unknown {sorted(names - expected)}"
                )
            for member in members:
                if not member.isfile():
                    raise ArtifactError(f"{path.name}: non-regular archive entry {member.name}")
                handle = archive.extractfile(member)
                if handle is None:
                    raise ArtifactError(f"{path.name}: cannot read {member.name}")
                found[member.name] = handle.read()
    return found


def aggregate(
    source: pathlib.Path,
    out: pathlib.Path,
    *,
    version: str,
    release_sha: str,
    epoch: int,
) -> tuple[pathlib.Path, ...]:
    expected = set()
    for target, (_binary, suffix, _static) in TARGETS.items():
        base = f"sipx-{version}-{target}"
        expected.update(
            {f"{base}{suffix}", f"{base}.build-manifest.json", f"{base}.spdx.json"}
        )
    actual = {path.name for path in source.iterdir() if path.is_file()}
    if actual != expected:
        raise ArtifactError(
            f"artifact set differs: missing {sorted(expected - actual)}, "
            f"unknown {sorted(actual - expected)}"
        )
    if any(path.is_dir() for path in source.iterdir()):
        raise ArtifactError("artifact input contains an unexpected directory")

    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise ArtifactError(f"aggregation output is not empty: {out}")
    checksum_paths = []
    copied = []
    for target, (binary_name, suffix, _static) in TARGETS.items():
        _validate_inputs(version, target, release_sha, epoch)
        base = f"sipx-{version}-{target}"
        manifest_path = source / f"{base}.build-manifest.json"
        sbom_path = source / f"{base}.spdx.json"
        archive_path = source / f"{base}{suffix}"
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            sbom = json.loads(sbom_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ArtifactError(f"cannot read target metadata for {target}: {error}") from error
        if not isinstance(manifest, dict) or not isinstance(sbom, dict):
            raise ArtifactError(f"target metadata for {target} is not an object")
        problems = manifest_problems(
            manifest,
            version=version,
            target=target,
            release_sha=release_sha,
            epoch=epoch,
        )
        problems.extend(
            spdx_problems(
                sbom,
                version=version,
                target=target,
                release_sha=release_sha,
                epoch=epoch,
            )
        )
        entries = _archive_entries(archive_path, target, version)
        archive_manifest = entries[f"{base}/build-manifest.json"]
        if archive_manifest != manifest_path.read_bytes():
            problems.append("archive build manifest differs from its sidecar")
        binary_hash = _sha256(entries[f"{base}/{binary_name}"])
        if manifest.get("binary_sha256") != binary_hash:
            problems.append("archived executable hash differs from the build manifest")
        if problems:
            raise ArtifactError(f"{target}: " + "\n".join(problems))
        # The manifest sidecar is matrix-to-aggregator evidence.  The release copy lives inside
        # the archive, so publishing a second unchecked copy would add an unnecessary asset.
        for path in (archive_path, sbom_path):
            destination = out / path.name
            shutil.copyfile(path, destination)
            copied.append(destination)
        checksum_paths.extend((out / archive_path.name, out / sbom_path.name))

    sums = out / "SHA256SUMS"
    sums.write_text(
        "".join(f"{_sha256(path.read_bytes())}  {path.name}\n" for path in sorted(checksum_paths)),
        encoding="utf-8",
    )
    copied.append(sums)
    return tuple(copied)


def compare_assets(
    expected: pathlib.Path, actual: pathlib.Path, *, allow_missing: bool = False
) -> list[str]:
    expected_files = {path.name: path for path in expected.iterdir() if path.is_file()}
    actual_files = {path.name: path for path in actual.iterdir() if path.is_file()}
    problems = []
    missing = set(expected_files) - set(actual_files)
    unknown = set(actual_files) - set(expected_files)
    if unknown or (missing and not allow_missing):
        problems.append(
            f"release assets differ: missing {sorted(missing)}, unknown {sorted(unknown)}"
        )
    for name in sorted(set(expected_files) & set(actual_files)):
        if _sha256(expected_files[name].read_bytes()) != _sha256(actual_files[name].read_bytes()):
            problems.append(f"existing release asset bytes differ: {name}")
    return problems


def _install_signal_handlers() -> None:
    def stop(signum: int, _frame: object) -> None:
        for process in tuple(_OWNED.values()):
            _terminate_tree(process)
            _close_process_pipes(process)
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    package_parser = subcommands.add_parser("package", help="smoke and package one native target")
    package_parser.add_argument("--binary", required=True, type=pathlib.Path)
    package_parser.add_argument("--out", required=True, type=pathlib.Path)
    package_parser.add_argument("--version", required=True)
    package_parser.add_argument("--target", required=True, choices=tuple(TARGETS))
    package_parser.add_argument("--release-sha", required=True)
    package_parser.add_argument("--source-date-epoch", required=True, type=int)
    package_parser.add_argument("--smoke-timeout", type=float, default=30.0)

    aggregate_parser = subcommands.add_parser("aggregate", help="validate the exact target set")
    aggregate_parser.add_argument("--input", required=True, type=pathlib.Path)
    aggregate_parser.add_argument("--out", required=True, type=pathlib.Path)
    aggregate_parser.add_argument("--version", required=True)
    aggregate_parser.add_argument("--release-sha", required=True)
    aggregate_parser.add_argument("--source-date-epoch", required=True, type=int)

    compare_parser = subcommands.add_parser("compare", help="compare existing release asset bytes")
    compare_parser.add_argument("--expected", required=True, type=pathlib.Path)
    compare_parser.add_argument("--actual", required=True, type=pathlib.Path)
    compare_parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="accept expected assets that have not been uploaded yet",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    _install_signal_handlers()
    try:
        if args.command == "package":
            paths = package(
                args.binary,
                args.out,
                version=args.version,
                target=args.target,
                release_sha=args.release_sha,
                epoch=args.source_date_epoch,
                smoke_timeout=args.smoke_timeout,
            )
            print("packaged release artifact: " + ", ".join(path.name for path in paths))
        elif args.command == "aggregate":
            paths = aggregate(
                args.input,
                args.out,
                version=args.version,
                release_sha=args.release_sha,
                epoch=args.source_date_epoch,
            )
            print(f"release artifacts: {len(paths)} files validated")
        else:
            problems = compare_assets(
                args.expected, args.actual, allow_missing=args.allow_missing
            )
            if problems:
                raise ArtifactError("\n".join(problems))
            print("release artifact bytes match")
    except (ArtifactError, OSError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"release artifacts refused: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
