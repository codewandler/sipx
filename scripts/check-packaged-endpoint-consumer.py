#!/usr/bin/env python3
"""Compile the registry-shaped endpoint consumer from packaged crate archives only."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tarfile
import tomllib
from typing import Any

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import api_compat


ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests" / "v1-endpoint-consumer"
SCRATCH = ROOT / "target" / "api-compat" / "package-rehearsal"
COMMAND_TIMEOUT_SECONDS = 300


class RehearsalError(RuntimeError):
    """The package boundary could not be established or compiled."""


def terminate_process_group(process: subprocess.Popen[str]) -> None:
    """Bound cleanup of Cargo and every compiler process it started."""
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.communicate(timeout=5)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.communicate()


def run(
    args: list[str], *, cwd: pathlib.Path, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    process = subprocess.Popen(
        args,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    previous_handlers: dict[signal.Signals, Any] = {}

    def interrupted(signum: int, _frame: Any) -> None:
        terminate_process_group(process)
        raise KeyboardInterrupt(f"interrupted by signal {signum}")

    for caught in (signal.SIGINT, signal.SIGTERM):
        previous_handlers[caught] = signal.getsignal(caught)
        signal.signal(caught, interrupted)
    try:
        try:
            stdout, stderr = process.communicate(timeout=COMMAND_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            terminate_process_group(process)
            raise RehearsalError(
                f"`{' '.join(args)}` exceeded the {COMMAND_TIMEOUT_SECONDS}s failure bound"
            ) from None
    finally:
        for caught, previous in previous_handlers.items():
            signal.signal(caught, previous)
    if process.returncode != 0:
        command = " ".join(args)
        raise RehearsalError(
            f"`{command}` failed in {cwd}:\n{stdout}{stderr}".rstrip()
        )
    return subprocess.CompletedProcess(args, process.returncode, stdout, stderr)


def dependency_name(alias: str, value: Any) -> str:
    if isinstance(value, str):
        return alias
    if isinstance(value, dict):
        return str(value.get("package", alias))
    raise RehearsalError(f"dependency `{alias}` has an invalid Cargo value")


def fixture_dependencies(manifest: pathlib.Path, version: str) -> set[str]:
    try:
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise RehearsalError(f"cannot read registry consumer manifest: {error}") from error
    if "workspace" in document or "patch" in document or "replace" in document:
        raise RehearsalError(
            "registry consumer manifest must not contain workspace, patch, or replace tables"
        )
    found: set[str] = set()
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        for alias, value in document.get(section, {}).items():
            name = dependency_name(alias, value)
            if not name.startswith("sipx-"):
                continue
            if isinstance(value, str):
                declared = value
            else:
                if any(key in value for key in ("path", "git", "workspace")):
                    raise RehearsalError(
                        f"registry consumer dependency `{alias}` is not registry-only"
                    )
                declared = value.get("version")
            if declared != f"={version}":
                raise RehearsalError(
                    f"registry consumer dependency `{alias}` must pin `={version}`, got {declared!r}"
                )
            found.add(name)
    if not found:
        raise RehearsalError("registry consumer has no sipx dependencies")
    return found


def package_closure(metadata: dict[str, Any], roots: set[str]) -> list[dict[str, Any]]:
    packages = {package["name"]: package for package in api_compat.publishable_packages(metadata)}
    unknown = sorted(roots - set(packages))
    if unknown:
        raise RehearsalError(f"consumer names unknown package(s): {', '.join(unknown)}")
    closure = set(roots)
    frontier = list(roots)
    while frontier:
        name = frontier.pop()
        for dependency in packages[name].get("dependencies", []):
            if dependency.get("kind") == "dev":
                continue
            dependency_name_ = dependency["name"]
            if dependency_name_ in packages and dependency_name_ not in closure:
                closure.add(dependency_name_)
                frontier.append(dependency_name_)
    return [packages[name] for name in sorted(closure)]


def safe_extract(archive: pathlib.Path, destination: pathlib.Path, expected_root: str) -> pathlib.Path:
    with tarfile.open(archive, "r:gz") as package:
        members = package.getmembers()
        if not members:
            raise RehearsalError(f"package archive {archive.name} is empty")
        for member in members:
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts:
                raise RehearsalError(f"package archive {archive.name} has unsafe path {member.name!r}")
            if path.parts[0] != expected_root:
                raise RehearsalError(
                    f"package archive {archive.name} has unexpected root {path.parts[0]!r}"
                )
            if member.issym() or member.islnk():
                raise RehearsalError(
                    f"package archive {archive.name} contains link {member.name!r}"
                )
        package.extractall(destination, members=members, filter="data")
    extracted = destination / expected_root
    if not (extracted / "Cargo.toml").is_file():
        raise RehearsalError(f"package archive {archive.name} has no normalized Cargo.toml")
    return extracted


def package_archives(packages: list[dict[str, Any]]) -> dict[str, pathlib.Path]:
    cargo_target = SCRATCH / "cargo-target"
    package_output = cargo_target / "package"
    archive_root = SCRATCH / "archives"
    if package_output.exists():
        shutil.rmtree(package_output)
    if archive_root.exists():
        shutil.rmtree(archive_root)
    archive_root.mkdir(parents=True)
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(cargo_target)
    command = ["cargo", "package"]
    for package in packages:
        command.extend(("-p", package["name"]))
    command.extend(
        (
            "--allow-dirty",
            "--no-verify",
            "--locked",
            "--offline",
            "--target-dir",
            str(cargo_target),
        )
    )
    # Cargo's package-set resolver prepares unpublished workspace dependencies together. Packaging
    # one member at a time would consult the registry for the not-yet-published release candidate
    # and either fail or, worse, rehearse an older dependency than the archive set under review.
    run(command, cwd=ROOT, env=environment)
    extracted: dict[str, pathlib.Path] = {}
    for package in packages:
        stem = f"{package['name']}-{package['version']}"
        archive = cargo_target / "package" / f"{stem}.crate"
        if not archive.is_file():
            raise RehearsalError(f"cargo package did not produce {archive}")
        extracted_path = safe_extract(archive, archive_root, stem)
        problem = api_compat.archive_patch_problem(ROOT, archive_root, extracted_path)
        if problem is not None:
            raise RehearsalError(problem)
        extracted[package["name"]] = extracted_path
    return extracted


def append_archive_patches(manifest: pathlib.Path, paths: dict[str, pathlib.Path]) -> None:
    text = manifest.read_text(encoding="utf-8")
    # The disposable consumer lives below the repository target directory but is intentionally a
    # separate workspace, just as the registry consumer will be.
    text += "\n[workspace]\n\n[patch.crates-io]\n"
    for name, path in sorted(paths.items()):
        text += f"{json.dumps(name)} = {{ path = {json.dumps(str(path))} }}\n"
    manifest.write_text(text, encoding="utf-8")


def seed_consumer_lockfile(
    consumer: pathlib.Path, workspace_lock: pathlib.Path = ROOT / "Cargo.lock"
) -> None:
    """Give the offline rehearsal the exact dependency graph of the release candidate."""
    if not workspace_lock.is_file():
        raise RehearsalError(f"workspace lockfile is absent: {workspace_lock}")
    shutil.copy2(workspace_lock, consumer / "Cargo.lock")


def lockfile_resolution_problem(
    workspace_lock: pathlib.Path, consumer_lock: pathlib.Path
) -> str | None:
    """Refuse any resolved package identity absent from the committed release graph."""
    try:
        workspace = tomllib.loads(workspace_lock.read_text(encoding="utf-8"))
        consumer = tomllib.loads(consumer_lock.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        return f"cannot compare release and consumer lockfiles: {error}"

    def identity(package: dict[str, Any]) -> tuple[str, str, str, str]:
        return (
            str(package.get("name", "")),
            str(package.get("version", "")),
            str(package.get("source", "")),
            str(package.get("checksum", "")),
        )

    released = {identity(package) for package in workspace.get("package", [])}
    unexpected = sorted(
        identity(package)
        for package in consumer.get("package", [])
        if package.get("name") != "sipx-v1-endpoint-consumer"
        and identity(package) not in released
    )
    if unexpected:
        rendered = ", ".join(
            f"{name} {version} ({source or 'path'})" for name, version, source, _ in unexpected
        )
        return f"consumer resolved package identities outside the workspace lock: {rendered}"
    return None


def prefetch_locked_workspace(
    environment: dict[str, str], workspace_root: pathlib.Path = ROOT
) -> None:
    """Populate a cold Cargo cache without permitting dependency resolution drift."""
    run(["cargo", "fetch", "--locked"], cwd=workspace_root, env=environment)


def check() -> None:
    if not (FIXTURE / "Cargo.toml").is_file() or not (FIXTURE / "src" / "main.rs").is_file():
        raise RehearsalError(
            f"registry consumer fixture is incomplete under {FIXTURE.relative_to(ROOT)}"
        )
    metadata = api_compat.workspace_metadata()
    packages = api_compat.publishable_packages(metadata)
    versions = sorted({package["version"] for package in packages})
    if len(versions) != 1:
        raise RehearsalError(f"publishable workspace versions disagree: {versions}")
    roots = fixture_dependencies(FIXTURE / "Cargo.toml", versions[0])
    closure = package_closure(metadata, roots)
    # `--offline` proves that the package-only consumer needs no live registry during resolution or
    # compilation; it must not accidentally require that an unrelated preceding build warmed every
    # target-specific archive in Cargo's cache. Fetch only identities from the committed lock first.
    prefetch_locked_workspace(os.environ.copy())
    paths = package_archives(closure)
    tree_digests_before = {
        name: api_compat.tree_digest(path) for name, path in sorted(paths.items())
    }

    archive_dir = SCRATCH / "cargo-target" / "package"
    digests_before = {
        archive.name: hashlib.sha256(archive.read_bytes()).hexdigest()
        for archive in sorted(archive_dir.glob("*.crate"))
    }
    if not digests_before:
        raise RehearsalError("package rehearsal observed no crate archives")

    consumer = SCRATCH / "consumer"
    if consumer.exists():
        shutil.rmtree(consumer)
    shutil.copytree(FIXTURE, consumer)
    append_archive_patches(consumer / "Cargo.toml", paths)
    # A fresh offline resolution can see a newly published version in the cached index without
    # having that version's archive cached. More importantly, it would no longer compile the exact
    # dependency graph whose lockfile accompanies this release candidate.
    seed_consumer_lockfile(consumer)
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(SCRATCH / "consumer-target")
    # Cargo prunes workspace-only packages when the lock is used by the smaller consumer. Permit
    # that mechanical rewrite once, then prove every retained identity came from the release lock
    # before compiling and inspecting the result under `--locked --offline`.
    run(
        ["cargo", "metadata", "--format-version=1", "--offline"],
        cwd=consumer,
        env=environment,
    )
    lock_problem = lockfile_resolution_problem(ROOT / "Cargo.lock", consumer / "Cargo.lock")
    if lock_problem is not None:
        raise RehearsalError(lock_problem)
    run(
        ["cargo", "check", "--all-targets", "--locked", "--offline"],
        cwd=consumer,
        env=environment,
    )
    resolved = run(
        ["cargo", "metadata", "--format-version=1", "--locked", "--offline"],
        cwd=consumer,
        env=environment,
    )
    try:
        consumer_metadata = json.loads(resolved.stdout)
    except json.JSONDecodeError as error:
        raise RehearsalError(f"consumer cargo metadata returned invalid JSON: {error}") from error
    resolution_problems = api_compat.archive_resolution_problems(
        consumer_metadata, paths, versions[0]
    )
    if resolution_problems:
        raise RehearsalError("; ".join(resolution_problems))

    digests_after = {
        archive.name: hashlib.sha256(archive.read_bytes()).hexdigest()
        for archive in sorted(archive_dir.glob("*.crate"))
    }
    if digests_before != digests_after:
        raise RehearsalError("a packaged archive changed during consumer compilation")
    tree_digests_after = {
        name: api_compat.tree_digest(path) for name, path in sorted(paths.items())
    }
    if tree_digests_before != tree_digests_after:
        raise RehearsalError("an extracted packaged source tree changed during consumer compilation")
    print(
        f"packaged endpoint consumer: {len(paths)} archive(s), registry-shaped fixture, locked offline compile"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", required=True)
    parser.parse_args()
    try:
        check()
        return 0
    except (RehearsalError, api_compat.ApiCompatError, OSError) as error:
        print(f"packaged endpoint consumer: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
