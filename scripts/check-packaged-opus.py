#!/usr/bin/env python3
"""Prove the normalized sipx packages carry a runnable opt-in Opus CLI.

This is deliberately separate from the registry consumer in ``release.py``.  It answers whether
the bytes Cargo would package *now* retain the feature chain; the release proof answers whether
published bytes at a clean tag are the same bytes and install from crates.io.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from collections.abc import Mapping, Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
NATIVE_PACKAGES = {"opus", "audiopus_sys"}
OWNED: dict[int, subprocess.Popen[bytes]] = {}


class ProofError(RuntimeError):
    """The package boundary did not prove the claimed feature behavior."""


def _terminate(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=2)
    except ProcessLookupError:
        return
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()


def _stop_owned(signum: int, _frame: object) -> None:
    for process in tuple(OWNED.values()):
        _terminate(process)
    raise SystemExit(128 + signum)


def run_bounded(
    command: Sequence[str],
    *,
    cwd: pathlib.Path,
    timeout: float,
    env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run one owned process group and reap every child on timeout or interruption."""

    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=None if env is None else dict(env),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    OWNED[process.pid] = process
    try:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            _terminate(process)
            stdout, stderr = process.communicate()
            complaint = stderr.decode("utf-8", errors="replace").strip()
            raise ProofError(
                f"command exceeded its {timeout:g}s bound: {' '.join(command)}"
                + (f"\n{complaint}" if complaint else "")
            ) from error
    finally:
        OWNED.pop(process.pid, None)
        _terminate(process)
    return subprocess.CompletedProcess(
        tuple(command),
        process.returncode,
        stdout.decode("utf-8", errors="replace"),
        stderr.decode("utf-8", errors="replace"),
    )


def archive_destination(prefix: str, member_name: str) -> pathlib.PurePosixPath:
    """Resolve an archive member below its one expected package prefix."""

    member = pathlib.PurePosixPath(member_name)
    if member.is_absolute() or not member.parts or member.parts[0] != prefix:
        raise ProofError(f"archive member leaves package prefix: {member_name}")
    relative = pathlib.PurePosixPath(*member.parts[1:])
    if not relative.parts or ".." in relative.parts:
        raise ProofError(f"archive member leaves package boundary: {member_name}")
    return relative


def extract_package(archive: pathlib.Path, destination: pathlib.Path) -> pathlib.Path:
    """Extract regular files without trusting tar paths or links."""

    prefix = archive.stem
    package_root = destination / prefix
    package_root.mkdir(parents=True)
    with tarfile.open(archive, mode="r:gz") as bundle:
        for member in bundle.getmembers():
            relative = archive_destination(prefix, member.name)
            target = package_root.joinpath(*relative.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise ProofError(f"archive member is not a regular file: {member.name}")
            source = bundle.extractfile(member)
            if source is None:
                raise ProofError(f"archive member cannot be read: {member.name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
    return package_root


def _feature(manifest: Mapping[str, object], name: str) -> set[str]:
    features = manifest.get("features", {})
    if not isinstance(features, dict):
        return set()
    values = features.get(name, [])
    if not isinstance(values, list):
        return set()
    return {str(value) for value in values}


def feature_chain_problems(manifests: Mapping[str, Mapping[str, object]]) -> list[str]:
    """Check the exact forwarding chain in Cargo's normalized manifests."""

    expected = {
        "sipx-cli": {"sipx-call/opus", "sipx-media/opus"},
        "sipx-call": {"sipx-media/opus"},
        "sipx-media": {"sipx-audio/opus"},
        "sipx-audio": {"dep:opus"},
    }
    problems: list[str] = []
    for package, required in expected.items():
        manifest = manifests.get(package)
        if manifest is None:
            problems.append(f"packaged feature proof has no {package} manifest")
            continue
        actual = _feature(manifest, "opus")
        missing = sorted(required - actual)
        if missing:
            problems.append(f"{package}/opus does not forward: {', '.join(missing)}")
        if _feature(manifest, "default"):
            problems.append(f"{package} enables a default feature; Opus must remain opt-in")
        package_table = manifest.get("package", {})
        if not isinstance(package_table, dict) or package_table.get("license") != "MIT OR Apache-2.0":
            problems.append(f"{package} archive does not carry the workspace SPDX licence")
    audio = manifests.get("sipx-audio", {})
    dependencies = audio.get("dependencies", {})
    opus = dependencies.get("opus", {}) if isinstance(dependencies, dict) else {}
    if not isinstance(opus, dict) or opus.get("optional") is not True:
        problems.append("sipx-audio's packaged native binding is not optional")
    return problems


def graph_problems(default_graph: str, opus_graph: str) -> list[str]:
    """Hold off-by-default and feature-on behavior against resolved package names."""

    def names(graph: str) -> set[str]:
        return {line.split()[0] for line in graph.splitlines() if line.split()}

    default_names = names(default_graph)
    opus_names = names(opus_graph)
    problems = []
    leaked = sorted(NATIVE_PACKAGES & default_names)
    if leaked:
        problems.append("default packaged CLI resolves native Opus packages: " + ", ".join(leaked))
    missing = sorted(NATIVE_PACKAGES - opus_names)
    if missing:
        problems.append("Opus packaged CLI does not resolve: " + ", ".join(missing))
    return problems


def help_problems(output: str) -> list[str]:
    """Require the packaged process to reach the real root command help."""

    # X-110 replaced the handwritten argument scanner, and the typed parser prints global options
    # ahead of the command. This assertion exists to prove the packaged process *reached* root help,
    # not to pin word order, so it tracks the parser's current line.
    if "USAGE:\n    sipx [OPTIONS] [COMMAND]" not in output:
        return ["clean packaged Opus CLI emitted no root help usage"]
    return []


def policy_documentation_problems(root: pathlib.Path) -> list[str]:
    """Require the public guide to expose the deliberate native/advisory boundary."""

    guide = (root / "website/docs/guides/as-a-library.md").read_text(encoding="utf-8")
    required = ("RUSTSEC-2026-0150", "audiopus_sys", "off by default", "MIT OR Apache-2.0")
    return [f"public Opus packaging note is missing {text!r}" for text in required if text not in guide]


def _require_success(result: subprocess.CompletedProcess[str], claim: str) -> str:
    if result.returncode != 0:
        complaint = result.stderr.strip() or result.stdout.strip() or f"status {result.returncode}"
        raise ProofError(f"{claim}: {complaint}")
    return result.stdout


def _workspace_public_packages(timeout: float) -> tuple[str, ...]:
    result = run_bounded(
        ("cargo", "metadata", "--format-version", "1", "--no-deps"),
        cwd=ROOT,
        timeout=timeout,
    )
    metadata = json.loads(_require_success(result, "cannot read workspace package metadata"))
    return tuple(
        sorted(
            str(package["name"])
            for package in metadata["packages"]
            if package.get("publish") != []
        )
    )


def _consumer_environment(root: pathlib.Path) -> dict[str, str]:
    environment = dict(os.environ)
    for name in tuple(environment):
        if name in {"CARGO_HOME", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"} or name.startswith(
            ("CARGO_SOURCE_", "CARGO_REGISTRIES_CRATES_IO_")
        ):
            del environment[name]
    environment["CARGO_HOME"] = str(root / "cargo-home")
    environment["CARGO_TARGET_DIR"] = str(root / "target")
    environment["CARGO_REGISTRIES_CRATES_IO_PROTOCOL"] = "sparse"
    return environment


def prove(timeout: float) -> None:
    public = _workspace_public_packages(timeout)
    with tempfile.TemporaryDirectory(prefix="sipx-packaged-opus-") as directory:
        root = pathlib.Path(directory)
        package_target = root / "package-target"
        command = [
            "cargo",
            "package",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "--target-dir",
            str(package_target),
            "--workspace",
        ]
        metadata = json.loads(
            _require_success(
                run_bounded(
                    ("cargo", "metadata", "--format-version", "1", "--no-deps"),
                    cwd=ROOT,
                    timeout=timeout,
                ),
                "cannot read private package metadata",
            )
        )
        for package in sorted(
            str(item["name"]) for item in metadata["packages"] if item.get("publish") == []
        ):
            command.extend(("--exclude", package))
        _require_success(
            run_bounded(command, cwd=ROOT, timeout=timeout),
            "cannot create normalized workspace packages",
        )

        unpacked = root / "packages"
        package_roots: dict[str, pathlib.Path] = {}
        manifests: dict[str, Mapping[str, object]] = {}
        for archive in sorted((package_target / "package").glob("*.crate")):
            package_root = extract_package(archive, unpacked)
            manifest = tomllib.loads((package_root / "Cargo.toml").read_text(encoding="utf-8"))
            package = manifest.get("package", {})
            if not isinstance(package, dict) or "name" not in package:
                raise ProofError(f"{archive.name} has no normalized package name")
            name = str(package["name"])
            package_roots[name] = package_root
            manifests[name] = manifest
        absent = sorted(set(public) - package_roots.keys())
        if absent:
            raise ProofError("Cargo did not produce public archives: " + ", ".join(absent))
        problems = feature_chain_problems(manifests)
        problems.extend(policy_documentation_problems(ROOT))
        if problems:
            raise ProofError("\n".join(problems))

        config = root / ".cargo" / "config.toml"
        config.parent.mkdir(parents=True)
        lines = ["[patch.crates-io]"]
        for name in public:
            lines.append(f"{name} = {{ path = {json.dumps(str(package_roots[name]))} }}")
        config.write_text("\n".join(lines) + "\n", encoding="utf-8")
        cli_manifest = package_roots["sipx-cli"] / "Cargo.toml"
        environment = _consumer_environment(root)
        _require_success(
            run_bounded(
                ("cargo", "generate-lockfile", "--manifest-path", str(cli_manifest)),
                cwd=root,
                timeout=timeout,
                env=environment,
            ),
            "clean packaged CLI consumer could not generate a lockfile",
        )
        common = (
            "cargo",
            "tree",
            "--locked",
            "--manifest-path",
            str(cli_manifest),
            "--no-default-features",
            "--edges",
            "normal",
            "--prefix",
            "none",
        )
        default_graph = _require_success(
            run_bounded(common, cwd=root, timeout=timeout, env=environment),
            "cannot resolve default packaged CLI graph",
        )
        opus_graph = _require_success(
            run_bounded(common + ("--features", "opus"), cwd=root, timeout=timeout, env=environment),
            "cannot resolve Opus packaged CLI graph",
        )
        problems = graph_problems(default_graph, opus_graph)
        if problems:
            raise ProofError("\n".join(problems))
        run = run_bounded(
            (
                "cargo",
                "run",
                "--locked",
                "--quiet",
                "--manifest-path",
                str(cli_manifest),
                "--no-default-features",
                "--features",
                "opus",
                "--",
                "--help",
            ),
            cwd=root,
            timeout=timeout,
            env=environment,
        )
        output = _require_success(run, "clean packaged Opus CLI did not build and run")
        problems = help_problems(output)
        if problems:
            raise ProofError("\n".join(problems))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="run the package proof")
    parser.add_argument("--timeout-seconds", type=float, default=300.0)
    args = parser.parse_args(argv)
    if not args.check:
        parser.error("--check is required")
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    for signum in (signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, _stop_owned)
    try:
        prove(args.timeout_seconds)
    except (OSError, ProofError, json.JSONDecodeError, tomllib.TOMLDecodeError) as error:
        print(f"packaged Opus: FAILED: {error}", file=sys.stderr)
        return 1
    print("packaged Opus: normalized feature-off and Opus CLI consumer pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
