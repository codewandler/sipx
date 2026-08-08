#!/usr/bin/env python3
"""Rehearse or perform Cargo registry publication in dependency order.

The default is a read-only graph and checkout check. See
`docs/specs/release-rehearsal.md` for the authority, safety boundary and restart semantics.
"""

from __future__ import annotations

import argparse
import datetime
import email.utils
import hashlib
import json
import os
import pathlib
import re
import selectors
import signal
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
from typing import Callable, Mapping, NamedTuple, Sequence

ROOT = pathlib.Path(__file__).resolve().parent.parent
CLI_PACKAGE = "sipx-cli"
CRATES_IO_LOCK_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
EXPECTED_GITHUB_REPOSITORY = "codewandler/sipx"
EXPECTED_GITHUB_WORKFLOW = ".github/workflows/crates-io.yml"
EXPECTED_GITHUB_RECOVERY_WORKFLOW = ".github/workflows/crates-io-resume.yml"
_OWNED_GROUPS: dict[int, subprocess.Popen[bytes]] = {}


class ReleaseError(ValueError):
    """The checkout cannot produce a registry release with the declared contract."""


class Dependency(NamedTuple):
    """One dependency as Cargo will retain it in a packaged manifest."""

    name: str
    requirement: str
    path: pathlib.Path | None
    kind: str | None
    source: str | None


class Package(NamedTuple):
    """The release-relevant part of one Cargo metadata package record."""

    name: str
    version: str
    public: bool
    dependencies: tuple[Dependency, ...]
    manifest: pathlib.Path
    readme: pathlib.Path | None
    license: str | None


class Visibility(NamedTuple):
    """Packages observed on the registry and packages still absent at the finite bound."""

    available: tuple[str, ...]
    missing: tuple[str, ...]


class ArchiveEvidence(NamedTuple):
    """The byte identity and clean Git identity embedded in one local Cargo archive."""

    checksum: str
    git_sha1: str
    dirty: bool


class RegistryRateLimit(NamedTuple):
    """One publication allowance the registry states, as the token bucket it operates."""

    burst: int
    refill_seconds: float


class RateLimitBucket(NamedTuple):
    """What this invocation currently believes one stated allowance permits, and from when.

    `level` is the number of uploads the allowance still permits as of `updated`; `not_before` is
    the moment the registry itself named in a refusal, which overrides the model whenever it is
    later than the model would be.
    """

    limit: RegistryRateLimit
    level: float
    updated: float
    not_before: float


class RateLimitRefusal(NamedTuple):
    """One registry rate-limit refusal, with the retry hint it did or did not carry."""

    retry_seconds: float | None
    detail: str


# crates.io states two publication limits, and they differ by an order of magnitude: a new crate
# name is allowed once every ten minutes after a burst of five, while a new version of a name that
# already exists is allowed once a minute after a burst of thirty. Modelling both is what keeps an
# ordinary version bump out of the new-name pacing; modelling one would be the constant delay this
# replaces.
NEW_CRATE_RATE_LIMIT = RegistryRateLimit(burst=5, refill_seconds=600.0)
NEW_VERSION_RATE_LIMIT = RegistryRateLimit(burst=30, refill_seconds=60.0)
NEW_CRATE = "new-crate"
NEW_VERSION = "new-version"
# The registry states one deadline per refusal. Three attempts covers a deadline that is read,
# waited out, and then restated once; beyond that the registry is refusing rather than pacing.
RATE_LIMIT_RETRY_ATTEMPTS = 3
RATE_LIMIT_SIGNALS = (
    re.compile(r"\bstatus:?\s+429\b", re.IGNORECASE),
    re.compile(r"\b429\s+too\s+many\s+requests\b", re.IGNORECASE),
    re.compile(r"\(429\)"),
)
RETRY_AFTER_HEADER = re.compile(
    r"^[^\S\n]*retry-after[^\S\n]*:[^\S\n]*(\S.*?)[^\S\n]*$", re.IGNORECASE | re.MULTILINE
)
RETRY_DEADLINE_ANCHOR = re.compile(r"try again (?:after|at)\b", re.IGNORECASE)
RFC_3339_MOMENT = re.compile(
    r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?"
)
HTTP_DATE_MOMENT = re.compile(
    r"[A-Za-z]{3},\s+\d{1,2}\s+[A-Za-z]{3}\s+\d{4}\s+\d{2}:\d{2}:\d{2}\s+[A-Za-z]{2,5}"
)


def package_records(
    records: Sequence[dict[str, object]], workspace_version: str, workspace_root: pathlib.Path
) -> tuple[Package, ...]:
    """Convert Cargo metadata records without silently supplying release facts.

    `workspace_version` is accepted here so fixture callers have to state the version whose graph
    they are constructing. Version disagreements are returned by `graph_problems`, not hidden by
    normalising the package records.
    """

    del workspace_version
    root = workspace_root.resolve()
    packages = []
    for record in records:
        manifest = pathlib.Path(str(record["manifest_path"])).resolve()
        raw_readme = record.get("readme")
        readme = None
        if isinstance(raw_readme, str):
            candidate = pathlib.Path(raw_readme)
            readme = candidate.resolve() if candidate.is_absolute() else (manifest.parent / candidate).resolve()
        dependencies = []
        for raw in record.get("dependencies", []):
            assert isinstance(raw, dict)
            raw_path = raw.get("path")
            dependency_path = pathlib.Path(str(raw_path)).resolve() if raw_path else None
            dependencies.append(
                Dependency(
                    name=str(raw["name"]),
                    requirement=str(raw.get("req", "*")),
                    path=dependency_path,
                    kind=str(raw["kind"]) if raw.get("kind") is not None else None,
                    source=str(raw["source"]) if raw.get("source") is not None else None,
                )
            )
        # Cargo serialises `publish = false` as an empty registry allow-list.
        public = record.get("publish") != []
        packages.append(
            Package(
                name=str(record["name"]),
                version=str(record["version"]),
                public=public,
                dependencies=tuple(dependencies),
                manifest=manifest,
                readme=readme,
                license=str(record["license"]) if record.get("license") else None,
            )
        )
    if not packages:
        raise ReleaseError(f"Cargo metadata returned no packages under {root}")
    return tuple(packages)


def _archive_dependencies(package: Package) -> tuple[Dependency, ...]:
    """Dependencies retained in the published manifest; dev-only edges are omitted by Cargo."""

    return tuple(dependency for dependency in package.dependencies if dependency.kind != "dev")


def _inside(path: pathlib.Path, root: pathlib.Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def _matching_requirement(requirement: str, version: str) -> bool:
    """Recognise Cargo's normalised spelling of a manifest's one-version requirement."""

    return requirement in {version, f"^{version}", f"={version}"}


def graph_problems(
    packages: Sequence[Package], workspace_version: str, workspace_root: pathlib.Path
) -> list[str]:
    """Return version, source and unpublished-dependency defects in the release graph."""

    by_name = {package.name: package for package in packages}
    problems = []
    for package in packages:
        if package.version != workspace_version:
            problems.append(
                f"{package.name}: package version {package.version} does not match workspace version "
                f"{workspace_version}"
            )
        if not package.public:
            continue
        for dependency in package.dependencies:
            if dependency.kind == "dev":
                target = by_name.get(dependency.name)
                if (
                    target is not None
                    and not target.public
                    and dependency.requirement != "*"
                ):
                    problems.append(
                        f"{package.name}: versioned dev-dependency {dependency.name} does not "
                        "publish; make unpublished test support path-only so Cargo omits it"
                    )
                continue
            if dependency.source and dependency.source.startswith("git+"):
                problems.append(
                    f"{package.name}: Git dependency {dependency.name} would escape the registry manifest"
                )
            if dependency.path is not None and not _inside(dependency.path, workspace_root):
                problems.append(
                    f"{package.name}: path dependency {dependency.name} points outside the workspace: "
                    f"{dependency.path}"
                )
            target = by_name.get(dependency.name)
            if target is None:
                if dependency.path is not None and _inside(dependency.path, workspace_root):
                    problems.append(
                        f"{package.name}: path dependency {dependency.name} is not a workspace package"
                    )
                continue
            if dependency.path is not None and dependency.path != target.manifest.parent.resolve():
                problems.append(
                    f"{package.name}: dependency {dependency.name} path {dependency.path} does not "
                    f"identify workspace package {target.manifest.parent.resolve()}"
                )
            if not _matching_requirement(dependency.requirement, workspace_version):
                problems.append(
                    f"{package.name}: workspace dependency {dependency.name} requirement "
                    f"{dependency.requirement!r} does not name {workspace_version}"
                )
            if not target.public:
                problems.append(
                    f"{package.name}: normal dependency {dependency.name} does not publish, so registry "
                    "consumers cannot resolve it"
                )
    return problems


def _license_files(expression: str) -> tuple[str, ...]:
    """Map this workspace's SPDX alternatives to their repository source-license files."""

    identifiers = set(re.findall(r"[A-Za-z][A-Za-z0-9.+-]*", expression))
    operators = {"AND", "OR", "WITH"}
    mapping = {"MIT": "LICENSE-MIT", "Apache-2.0": "LICENSE-APACHE"}
    return tuple(sorted(mapping.get(identifier, f"LICENSE-{identifier}") for identifier in identifiers - operators))


def metadata_problems(
    packages: Sequence[Package], workspace_root: pathlib.Path, workspace_license: str
) -> list[str]:
    """Return missing README and license metadata/files for public packages."""

    problems = []
    for package in packages:
        if not package.public:
            continue
        if package.readme is None:
            problems.append(f"{package.name}: public package has no README metadata")
        elif not package.readme.is_file():
            problems.append(f"{package.name}: README does not exist: {package.readme}")
        if not package.license:
            problems.append(f"{package.name}: public package has no SPDX license expression")
        elif package.license != workspace_license:
            problems.append(
                f"{package.name}: license {package.license!r} does not match workspace license "
                f"{workspace_license!r}"
            )
    for filename in _license_files(workspace_license):
        if not (workspace_root / filename).is_file():
            problems.append(f"workspace license file is absent: {filename}")
    return problems


def archive_listing_problems(package: Package, entries: Sequence[str]) -> list[str]:
    """Return missing package front doors and paths that leave the archive boundary."""

    problems = []
    names = set(entries)
    for required in ("Cargo.toml", "Cargo.toml.orig"):
        if required not in names:
            problems.append(f"{package.name}: package listing omits {required}")
    if package.readme is None:
        problems.append(f"{package.name}: package listing has no declared README to check")
    else:
        try:
            readme = package.readme.relative_to(package.manifest.parent).as_posix()
        except ValueError:
            problems.append(f"{package.name}: declared README escapes the package directory")
        else:
            if readme not in names:
                problems.append(f"{package.name}: package listing omits declared README {readme}")
    for entry in entries:
        path = pathlib.PurePosixPath(entry)
        if path.is_absolute() or ".." in path.parts or not path.parts:
            problems.append(f"{package.name}: package entry escapes its archive boundary: {entry}")
        if any(part in {".git", "target"} for part in path.parts):
            problems.append(f"{package.name}: package entry includes workspace-only state: {entry}")
    return problems


def _dependency_sections(manifest: dict[str, object]) -> tuple[tuple[str, dict[str, object]], ...]:
    """Find normal/build/dev dependency tables, including target-specific forms."""

    found = []
    for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
        table = manifest.get(kind)
        if isinstance(table, dict):
            found.append((kind, table))
    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
                table = target.get(kind)
                if isinstance(table, dict):
                    found.append((kind, table))
    return tuple(found)


def normalized_manifest_problems(
    package: Package,
    text: str,
    *,
    workspace_packages: set[str],
    public_packages: set[str],
) -> list[str]:
    """Validate release metadata and dependency sources in Cargo's normalized manifest."""

    try:
        manifest = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        return [f"{package.name}: normalized Cargo.toml is invalid: {error}"]
    raw_package = manifest.get("package")
    if not isinstance(raw_package, dict):
        return [f"{package.name}: normalized Cargo.toml has no package table"]
    problems = []
    for key, expected in (
        ("name", package.name),
        ("version", package.version),
        ("license", package.license),
    ):
        if raw_package.get(key) != expected:
            problems.append(
                f"{package.name}: normalized package {key} {raw_package.get(key)!r} does not match "
                f"{expected!r}"
            )
    if package.readme is not None:
        try:
            expected_readme = package.readme.relative_to(package.manifest.parent).as_posix()
        except ValueError:
            expected_readme = None
        if expected_readme is not None and raw_package.get("readme") != expected_readme:
            problems.append(
                f"{package.name}: normalized package README {raw_package.get('readme')!r} does not "
                f"match {expected_readme!r}"
            )
    for kind, dependencies in _dependency_sections(manifest):
        for dependency_name, raw in dependencies.items():
            if not isinstance(raw, dict):
                continue
            actual_name = str(raw.get("package", dependency_name))
            if "path" in raw:
                problems.append(
                    f"{package.name}: normalized {kind} dependency {actual_name} retains a path"
                )
            if "git" in raw:
                problems.append(
                    f"{package.name}: normalized {kind} dependency {actual_name} retains a Git source"
                )
            if actual_name in workspace_packages and actual_name not in public_packages:
                problems.append(
                    f"{package.name}: normalized {kind} dependency {actual_name} names an "
                    "unpublished workspace package"
                )
    return problems


def _archive_manifest(
    package: Package, archive: pathlib.Path
) -> tuple[str | None, tuple[str, ...], list[str]]:
    """Read the normalized manifest without extracting an untrusted archive."""

    prefix = f"{package.name}-{package.version}/"
    problems = []
    manifest_text = None
    entries = []
    with tarfile.open(archive, mode="r:gz") as bundle:
        for member in bundle.getmembers():
            if not member.name.startswith(prefix):
                problems.append(
                    f"{package.name}: archive member is outside the package prefix: {member.name}"
                )
                continue
            relative = member.name.removeprefix(prefix)
            entries.append(relative)
            relative_path = pathlib.PurePosixPath(relative)
            if relative_path.is_absolute() or ".." in relative_path.parts:
                problems.append(
                    f"{package.name}: archive member escapes its package boundary: {member.name}"
                )
            if member.issym() or member.islnk():
                link = pathlib.PurePosixPath(member.linkname)
                if link.is_absolute() or ".." in link.parts:
                    problems.append(
                        f"{package.name}: archive link escapes its package boundary: "
                        f"{relative} -> {member.linkname}"
                    )
            if relative == "Cargo.toml":
                stream = bundle.extractfile(member)
                if stream is None:
                    problems.append(f"{package.name}: normalized Cargo.toml is not a regular file")
                else:
                    manifest_text = stream.read().decode("utf-8")
    if manifest_text is None:
        problems.append(f"{package.name}: archive has no normalized Cargo.toml")
    return manifest_text, tuple(entries), problems


def inspect_contents(
    packages: Sequence[Package], workspace_root: pathlib.Path = ROOT
) -> list[str]:
    """Inspect Cargo's real dirty-tree listings and normalized local archives."""

    problems = []
    workspace_packages = {package.name for package in packages}
    public_packages = {package.name for package in packages if package.public}
    public = sorted((item for item in packages if item.public), key=lambda item: item.name)
    listings = {}
    for package in public:
        listing_result = _capture(
            ("cargo", "package", "--locked", "--allow-dirty", "--list", "-p", package.name),
            root=workspace_root,
        )
        if listing_result.returncode != 0:
            problems.append(
                f"{package.name}: cargo package --list failed: "
                + (listing_result.stderr.strip() or f"status {listing_result.returncode}")
            )
            continue
        listing = tuple(line for line in listing_result.stdout.splitlines() if line)
        listings[package.name] = listing
        problems.extend(archive_listing_problems(package, listing))

    # Workspace packaging makes Cargo stage its own temporary registry in dependency order. That
    # proves a dependent archive without requiring this prerelease version to exist on crates.io.
    # Path-only workspace fixture dependencies disappear during Cargo's normalization before any
    # public archive is written, so package consumers do not inherit the repository's test graph.
    with tempfile.TemporaryDirectory(prefix="sipx-package-workspace-") as directory:
        target = pathlib.Path(directory)
        command = [
            "cargo",
            "package",
            "--locked",
            "--allow-dirty",
            "--no-verify",
            "--target-dir",
            str(target),
            "--workspace",
        ]
        for private in sorted(package.name for package in packages if not package.public):
            command.extend(("--exclude", private))
        package_result = _capture(tuple(command), root=workspace_root)
        if package_result.returncode != 0:
            problems.append(
                "workspace local archive creation failed: "
                + (package_result.stderr.strip() or f"status {package_result.returncode}")
            )
        for package in public:
            archive = target / "package" / f"{package.name}-{package.version}.crate"
            if not archive.is_file():
                problems.append(f"{package.name}: Cargo did not create expected archive {archive}")
                continue
            try:
                normalized, archive_entries, archive_problems = _archive_manifest(package, archive)
            except (OSError, tarfile.TarError, UnicodeDecodeError) as error:
                problems.append(f"{package.name}: cannot read local archive: {error}")
                continue
            problems.extend(archive_problems)
            listing = listings.get(package.name, ())
            if listing and set(archive_entries) != set(listing):
                missing = sorted(set(listing) - set(archive_entries))
                extra = sorted(set(archive_entries) - set(listing))
                problems.append(
                    f"{package.name}: Cargo listing and local archive differ; "
                    f"missing={missing}, extra={extra}"
                )
            if normalized is not None:
                problems.extend(
                    normalized_manifest_problems(
                        package,
                        normalized,
                        workspace_packages=workspace_packages,
                        public_packages=public_packages,
                    )
                )
    return problems


def _extract_package_source(archive: pathlib.Path, destination: pathlib.Path) -> pathlib.Path:
    """Extract one Cargo archive without trusting archive paths or links."""

    with tarfile.open(archive, mode="r:gz") as bundle:
        members = bundle.getmembers()
        prefixes = {pathlib.PurePosixPath(member.name).parts[0] for member in members if member.name}
        if len(prefixes) != 1:
            raise ReleaseError(f"{archive.name}: package archive has no single source prefix")
        prefix = next(iter(prefixes))
        root = destination / prefix
        for member in members:
            path = pathlib.PurePosixPath(member.name)
            if path.is_absolute() or not path.parts or path.parts[0] != prefix or ".." in path.parts:
                raise ReleaseError(f"{archive.name}: package member escapes its source: {member.name}")
            relative = pathlib.Path(*path.parts[1:])
            target = root / relative
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise ReleaseError(
                    f"{archive.name}: package member is not a regular file: {member.name}"
                )
            target.parent.mkdir(parents=True, exist_ok=True)
            source = bundle.extractfile(member)
            if source is None:
                raise ReleaseError(f"{archive.name}: cannot read package member {member.name}")
            with target.open("wb") as output:
                shutil.copyfileobj(source, output)
        return root


def local_package_set_members(
    packages: Sequence[Package], root: str = "sipx-testkit"
) -> tuple[str, ...]:
    """Return one public package and its complete workspace dependency closure in release order."""

    public = {package.name: package for package in packages if package.public}
    if root not in public:
        raise ReleaseError(f"local package-set root is not public: {root}")
    members: set[str] = set()
    pending = [root]
    while pending:
        name = pending.pop()
        if name in members:
            continue
        members.add(name)
        pending.extend(_public_dependencies(public[name], public))
    return tuple(name for name in publication_order(packages) if name in members)


def local_package_consumer_manifest(
    version: str, sources: Mapping[str, pathlib.Path]
) -> str:
    """A clean consumer of the exact locally staged package dependency closure."""

    testkit_source = sources.get("sipx-testkit")
    if testkit_source is None:
        raise ReleaseError("local package set omits sipx-testkit")
    testkit = json.dumps(str(testkit_source.resolve()))
    patches = "\n".join(
        f'{name} = {{ version = "={version}", path = {json.dumps(str(source.resolve()))} }}'
        for name, source in sources.items()
        if name != "sipx-testkit"
    )
    return f"""[package]
name = "sipx-local-package-example"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
sipx-testkit = {{ version = "={version}", path = {testkit} }}
tokio = {{ version = "1", features = ["macros", "rt"] }}

[patch.crates-io]
{patches}
"""


def local_package_lock_problems(
    lock: Mapping[str, object], version: str, members: Sequence[str]
) -> list[str]:
    """Prove every package-set member came from staged paths, not an older registry release."""

    records = [item for item in lock.get("package", []) if isinstance(item, dict)]
    problems = []
    for name in members:
        matches = [
            item for item in records if item.get("name") == name and item.get("version") == version
        ]
        if len(matches) != 1:
            problems.append(f"{name}: clean consumer did not resolve one exact {version} package")
        elif matches[0].get("source") is not None:
            problems.append(f"{name}: clean consumer resolved the registry instead of staged bytes")
    return problems


def supported_test_surface_example(source: pathlib.Path) -> pathlib.Path:
    """The packaged example which proves the public test-product surface."""

    manifest_path = source / "Cargo.toml"
    try:
        package = tomllib.loads(manifest_path.read_text(encoding="utf-8"))["package"]
        declaration = package["metadata"]["sipx-supported-test-surface"]
        example = declaration["example"]
    except (OSError, KeyError, TypeError, tomllib.TOMLDecodeError) as error:
        raise ReleaseError(
            "sipx-testkit package omits valid supported test-surface metadata"
        ) from error
    if (
        not isinstance(declaration, dict)
        or set(declaration) != {"example"}
        or not isinstance(example, str)
        or re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]*", example) is None
    ):
        raise ReleaseError("sipx-testkit package has invalid supported test-surface metadata")
    path = source / "examples" / f"{example}.rs"
    if not path.is_file():
        raise ReleaseError(f"sipx-testkit package omits examples/{example}.rs")
    return path


def verify_local_rtp_echo_package_set(
    packages: Sequence[Package],
    version: str,
    *,
    package_timeout: float,
    consumer_timeout: float,
    workspace_root: pathlib.Path = ROOT,
) -> None:
    """Package the complete testkit dependency closure and compile its example as a consumer."""

    required = local_package_set_members(packages)

    with tempfile.TemporaryDirectory(prefix="sipx-local-package-set-") as directory:
        root = pathlib.Path(directory)
        target = root / "package-target"
        command = [
            "cargo",
            "package",
            "--locked",
            "--allow-dirty",
            "--target-dir",
            str(target),
        ]
        for name in required:
            command.extend(("-p", name))
        packaged = _bounded_run(
            tuple(command),
            cwd=workspace_root,
            timeout=package_timeout,
        )
        if packaged.returncode != 0:
            raise ReleaseError(
                "dependency-ordered RTP echo package verification failed: "
                + (packaged.stderr.strip() or f"status {packaged.returncode}")
            )

        staged = root / "staged"
        sources = {}
        for name in required:
            archive = target / "package" / f"{name}-{version}.crate"
            if not archive.is_file():
                raise ReleaseError(f"{name}: Cargo did not create expected package {archive}")
            sources[name] = _extract_package_source(archive, staged)

        example = supported_test_surface_example(sources["sipx-testkit"])
        consumer = root / "consumer"
        (consumer / "src").mkdir(parents=True)
        (consumer / "Cargo.toml").write_text(
            local_package_consumer_manifest(version, sources),
            encoding="utf-8",
        )
        shutil.copyfile(example, consumer / "src" / "main.rs")
        environment = consumer_environment(root / "cargo-home")
        generated = _bounded_run(
            ("cargo", "generate-lockfile"),
            cwd=consumer,
            timeout=consumer_timeout,
            env=environment,
        )
        if generated.returncode != 0:
            raise ReleaseError(
                "local package-set consumer could not resolve: "
                + (generated.stderr.strip() or f"status {generated.returncode}")
            )
        lock = tomllib.loads((consumer / "Cargo.lock").read_text(encoding="utf-8"))
        lock_problems = local_package_lock_problems(lock, version, required)
        if lock_problems:
            raise ReleaseError("\n".join(lock_problems))
        checked = _bounded_run(
            ("cargo", "check", "--locked", "--all-targets"),
            cwd=consumer,
            timeout=consumer_timeout,
            env=environment,
        )
        if checked.returncode != 0:
            raise ReleaseError(
                "exact staged RTP echo example failed in a clean consumer: "
                + (checked.stderr.strip() or f"status {checked.returncode}")
            )


def _public_dependencies(package: Package, by_name: dict[str, Package]) -> set[str]:
    return {
        dependency.name
        for dependency in _archive_dependencies(package)
        if dependency.name in by_name and by_name[dependency.name].public
    }


def publication_order(packages: Sequence[Package]) -> tuple[str, ...]:
    """Return the deterministic dependency-first order of public packages."""

    public = {package.name: package for package in packages if package.public}
    remaining = {name: _public_dependencies(package, public) for name, package in public.items()}
    result = []
    while remaining:
        ready = sorted(name for name, dependencies in remaining.items() if not dependencies)
        if not ready:
            members = ", ".join(sorted(remaining))
            raise ReleaseError(f"public package dependency cycle among: {members}")
        # Remove one node at a time. A node released by this removal joins the same lexical choice as
        # nodes that were already ready, rather than being delayed behind an arbitrary Kahn "layer".
        name = ready[0]
        result.append(name)
        del remaining[name]
        for dependencies in remaining.values():
            dependencies.discard(name)
    return tuple(result)


def ready_frontier(packages: Sequence[Package], available: set[str]) -> tuple[str, ...]:
    """Return missing packages whose public dependencies are already registry-visible."""

    public = {package.name: package for package in packages if package.public}
    return tuple(
        name
        for name in publication_order(packages)
        if name not in available and _public_dependencies(public[name], public) <= available
    )


def recovery_visibility_problems(
    authorization: str | None, available: Sequence[str]
) -> list[str]:
    """Keep recovery authority narrower than authority to begin a publication."""

    if authorization is not None and not available:
        return [
            "CI recovery requires at least one exact workspace package to be already "
            "registry-visible; it cannot authorize first publication"
        ]
    return []


def checkout_problems(
    mode: str,
    version: str,
    *,
    dirty: bool,
    tags: Sequence[str],
    confirmation: str | None,
    ci: bool,
    annotated_tags: Sequence[str] = (),
    head_sha: str | None = None,
    ci_authorization: str | None = None,
    ci_recovery_authorization: str | None = None,
    controller_sha: str | None = None,
    ci_environment: Mapping[str, str] | None = None,
) -> list[str]:
    """Return checkout/authority defects before any Cargo publication command can run."""

    problems = []
    if dirty:
        problems.append("release checkout must be clean")
    if mode not in {"publish", "verify-consumer"}:
        return problems
    tag = f"v{version}"
    if tuple(tags) != (tag,):
        problems.append(f"{mode} requires HEAD to carry only the exact tag {tag}")
    elif tuple(annotated_tags) != (tag,):
        problems.append(f"{mode} requires {tag} to be an annotated tag")
    if mode == "publish" and confirmation != tag:
        problems.append(f"publish requires the exact confirmation --confirm-publish {tag}")
    if mode == "publish" and ci:
        environment = {} if ci_environment is None else ci_environment
        if ci_authorization is not None and ci_recovery_authorization is not None:
            problems.append("CI publication accepts only one authorization mode")
        elif ci_recovery_authorization is not None:
            problems.extend(
                ci_recovery_problems(
                    version,
                    release_sha=head_sha,
                    controller_sha=controller_sha,
                    authorization=ci_recovery_authorization,
                    environment=environment,
                )
            )
        else:
            problems.extend(
                ci_publish_problems(
                    version,
                    head_sha=head_sha,
                    authorization=ci_authorization,
                    environment=environment,
                )
            )
    elif mode == "publish":
        if ci_authorization is not None:
            problems.append("--authorize-ci-publish is valid only inside authorized GitHub Actions")
        if ci_recovery_authorization is not None:
            problems.append("--authorize-ci-recovery is valid only inside authorized GitHub Actions")
    return problems


def ci_publish_problems(
    version: str,
    *,
    head_sha: str | None,
    authorization: str | None,
    environment: Mapping[str, str],
) -> list[str]:
    """Return reasons a CI process lacks authority to publish a registry frontier."""

    tag = f"v{version}"
    sha = head_sha or ""
    problems = []
    exact = {
        "CI": "true",
        "GITHUB_ACTIONS": "true",
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_REPOSITORY": EXPECTED_GITHUB_REPOSITORY,
        "GITHUB_REF": f"refs/tags/{tag}",
        "GITHUB_REF_TYPE": "tag",
        "GITHUB_REF_NAME": tag,
    }
    for name, expected in exact.items():
        if environment.get(name) != expected:
            problems.append(f"CI publish requires {name}={expected!r}")

    event = environment.get("GITHUB_EVENT_NAME")
    if event not in {"push", "workflow_dispatch"}:
        problems.append("CI publish requires a tag push or tag-selected workflow_dispatch event")

    if re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        problems.append("CI publish requires HEAD to be one full lowercase Git object ID")
    else:
        if environment.get("GITHUB_SHA") != sha:
            problems.append("CI publish requires GITHUB_SHA to equal checked-out HEAD")
        if environment.get("GITHUB_WORKFLOW_SHA") != sha:
            problems.append("CI publish requires GITHUB_WORKFLOW_SHA to equal checked-out HEAD")
        expected_authorization = f"{tag}@{sha}"
        if authorization != expected_authorization:
            problems.append(
                "CI publish requires the exact commit authorization "
                f"--authorize-ci-publish {expected_authorization}"
            )

    workflow_ref = environment.get("GITHUB_WORKFLOW_REF", "")
    expected_workflow_ref = (
        f"{EXPECTED_GITHUB_REPOSITORY}/{EXPECTED_GITHUB_WORKFLOW}@refs/tags/{tag}"
    )
    if workflow_ref != expected_workflow_ref:
        problems.append(f"CI publish requires GITHUB_WORKFLOW_REF={expected_workflow_ref!r}")

    for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        value = environment.get(name, "")
        if not value.isascii() or not value.isdigit() or int(value) <= 0:
            problems.append(f"CI publish requires a positive numeric {name}")
    if not environment.get("CARGO_REGISTRY_TOKEN", "").strip():
        problems.append("CI publish requires CARGO_REGISTRY_TOKEN")
    return problems


def ci_recovery_problems(
    version: str,
    *,
    release_sha: str | None,
    controller_sha: str | None,
    authorization: str | None,
    environment: Mapping[str, str],
) -> list[str]:
    """Return reasons a main-branch recovery run lacks narrowly bound authority."""

    tag = f"v{version}"
    release = release_sha or ""
    controller = controller_sha or ""
    problems = []
    exact = {
        "CI": "true",
        "GITHUB_ACTIONS": "true",
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_REPOSITORY": EXPECTED_GITHUB_REPOSITORY,
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_REF_TYPE": "branch",
        "GITHUB_REF_NAME": "main",
    }
    for name, expected in exact.items():
        if environment.get(name) != expected:
            problems.append(f"CI recovery requires {name}={expected!r}")

    if re.fullmatch(r"[0-9a-f]{40}", release) is None:
        problems.append("CI recovery requires the release HEAD to be one full lowercase Git object ID")
    if re.fullmatch(r"[0-9a-f]{40}", controller) is None:
        problems.append("CI recovery requires the controller to be one full lowercase Git object ID")
    else:
        if environment.get("GITHUB_SHA") != controller:
            problems.append("CI recovery requires GITHUB_SHA to equal the controller checkout HEAD")
        if environment.get("GITHUB_WORKFLOW_SHA") != controller:
            problems.append(
                "CI recovery requires GITHUB_WORKFLOW_SHA to equal the controller checkout HEAD"
            )

    workflow_ref = environment.get("GITHUB_WORKFLOW_REF", "")
    expected_workflow_ref = (
        f"{EXPECTED_GITHUB_REPOSITORY}/{EXPECTED_GITHUB_RECOVERY_WORKFLOW}@refs/heads/main"
    )
    if workflow_ref != expected_workflow_ref:
        problems.append(f"CI recovery requires GITHUB_WORKFLOW_REF={expected_workflow_ref!r}")

    failed_run_id = environment.get("SIPX_FAILED_RELEASE_RUN_ID", "")
    if not failed_run_id.isascii() or not failed_run_id.isdigit() or int(failed_run_id) <= 0:
        problems.append("CI recovery requires one positive numeric failed release run ID")
    authorization_match = re.fullmatch(
        r"(v[^@]+)@([0-9a-f]{40})@([1-9][0-9]*)", authorization or ""
    )
    if authorization_match is None or authorization_match.group(1, 2) != (tag, release):
        problems.append(
            "CI recovery requires the exact recovery authorization "
            f"--authorize-ci-recovery {tag}@{release}@{failed_run_id}"
        )
    elif authorization_match.group(3) != failed_run_id:
        problems.append("CI recovery authorization must name the exact failed release run")

    run_id = environment.get("GITHUB_RUN_ID", "")
    if not run_id.isascii() or not run_id.isdigit() or int(run_id) <= 0:
        problems.append("CI recovery requires a positive numeric GITHUB_RUN_ID")
    elif run_id == failed_run_id:
        problems.append("CI recovery requires the current recovery run to differ from the failed run")
    attempt = environment.get("GITHUB_RUN_ATTEMPT", "")
    if not attempt.isascii() or not attempt.isdigit() or int(attempt) <= 0:
        problems.append("CI recovery requires a positive numeric GITHUB_RUN_ATTEMPT")
    if not environment.get("CARGO_REGISTRY_TOKEN", "").strip():
        problems.append("CI recovery requires CARGO_REGISTRY_TOKEN")
    return problems


def commands_for(
    mode: str, order: Sequence[str], *, excluded: Sequence[str] = ()
) -> tuple[tuple[str, ...], ...]:
    """Build the only Cargo write candidates; check and dry-run stay mechanically distinct."""

    if mode == "check":
        return ()
    if mode == "dry-run":
        command = [
            "cargo",
            "publish",
            "--registry",
            "crates-io",
            "--dry-run",
            "--locked",
            "--workspace",
        ]
        for package in sorted(excluded):
            command.extend(("--exclude", package))
        return (tuple(command),)
    if mode == "publish":
        return tuple(
            ("cargo", "publish", "--registry", "crates-io", "--locked", "-p", package)
            for package in order
        )
    raise ReleaseError(f"unknown release mode {mode!r}")


def _capture(
    command: Sequence[str], timeout: float = 120.0, *, root: pathlib.Path = ROOT
) -> subprocess.CompletedProcess[str]:
    return _bounded_run(command, cwd=root, timeout=timeout)


def _dispatch(
    command: Sequence[str], root: pathlib.Path, timeout: float
) -> subprocess.CompletedProcess[str]:
    """Run one Cargo write candidate, echoing exactly what the registry answered."""

    print("+ " + " ".join(command))
    result = _bounded_run(tuple(command), cwd=root, timeout=timeout)
    if result.stdout:
        print(result.stdout, end="")
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr)
    return result


def _metadata(root: pathlib.Path = ROOT) -> dict[str, object]:
    result = _capture(
        ("cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"), root=root
    )
    if result.returncode != 0:
        raise ReleaseError(result.stderr.strip() or "cargo metadata failed")
    return json.loads(result.stdout)


def _checkout(root: pathlib.Path = ROOT) -> tuple[bool, tuple[str, ...], tuple[str, ...]]:
    status = _capture(("git", "status", "--porcelain", "--untracked-files=all"), root=root)
    if status.returncode != 0:
        raise ReleaseError(status.stderr.strip() or "git status failed")
    tags = _capture(("git", "tag", "--points-at", "HEAD"), root=root)
    if tags.returncode != 0:
        raise ReleaseError(tags.stderr.strip() or "git tag inspection failed")
    names = tuple(sorted(tags.stdout.splitlines()))
    annotated = []
    for name in names:
        kind = _capture(("git", "cat-file", "-t", f"refs/tags/{name}"), root=root)
        if kind.returncode != 0:
            raise ReleaseError(kind.stderr.strip() or f"cannot inspect tag {name}")
        if kind.stdout.strip() == "tag":
            annotated.append(name)
    return bool(status.stdout.strip()), names, tuple(annotated)


def _head_commit(root: pathlib.Path = ROOT) -> str:
    """Return the full commit checked out by a CI publication run."""

    result = _capture(("git", "rev-parse", "HEAD"), root=root)
    if result.returncode != 0:
        raise ReleaseError(result.stderr.strip() or "cannot read release commit")
    head = result.stdout.strip()
    if re.fullmatch(r"[0-9a-f]{40}", head) is None:
        raise ReleaseError(f"release commit is not a full Git object ID: {head!r}")
    return head


def poll_registry_visibility(
    packages: Sequence[str],
    probe: Callable[[str, float], bool],
    *,
    timeout: float,
    interval: float,
    monotonic: Callable[[], float] = time.monotonic,
    pause: Callable[[float], None] = time.sleep,
) -> Visibility:
    """Poll exact versions without allowing a registry outage to wait forever."""

    if timeout <= 0 or interval <= 0:
        raise ReleaseError("registry visibility timeout and interval must be greater than zero")
    deadline = monotonic() + timeout
    missing = set(packages)
    while missing:
        for package in sorted(missing):
            remaining = deadline - monotonic()
            if remaining <= 0:
                break
            if probe(package, remaining):
                missing.remove(package)
        if not missing:
            break
        remaining = deadline - monotonic()
        if remaining <= 0:
            break
        pause(min(interval, remaining))
    available = set(packages) - missing
    return Visibility(tuple(sorted(available)), tuple(sorted(missing)))


def _registry_available(
    package: str, version: str, timeout: float = 15.0, root: pathlib.Path = ROOT
) -> bool:
    result = _bounded_run(
        (
            "cargo",
            "info",
            f"{package}@{version}",
            "--registry",
            "crates-io",
            "--color",
            "never",
        ),
        cwd=root,
        timeout=max(0.1, timeout),
    )
    if result.returncode == 0:
        return True
    exact_not_found = (
        f"error: could not find `{package}@{version}` in registry "
        "`https://github.com/rust-lang/crates.io-index`"
    )
    lines = [line.strip() for line in result.stderr.splitlines() if line.strip()]
    errors = [line for line in lines if line.startswith(("error:", "warning:"))]
    if result.returncode == 101 and errors == [exact_not_found]:
        return False
    complaint = result.stderr.strip() or f"status {result.returncode}"
    raise ReleaseError(f"crates.io probe failed for {package}@{version}: {complaint}")


def _registry_name_exists(
    package: str, timeout: float = 15.0, root: pathlib.Path = ROOT
) -> bool | None:
    """Ask whether the registry already carries this crate name under any version.

    `None` means the registry gave no readable answer. Unlike the exact-version probe, this one
    selects only which stated limit paces an upload and can neither skip nor repeat one, so an
    unreadable answer paces conservatively instead of refusing a release: the version probe still
    decides what is published, and a name read wrongly is corrected by the registry's own `429`.
    """

    result = _bounded_run(
        ("cargo", "info", package, "--registry", "crates-io", "--color", "never"),
        cwd=root,
        timeout=max(0.1, timeout),
    )
    if result.returncode == 0:
        return True
    exact_not_found = (
        f"error: could not find `{package}` in registry "
        "`https://github.com/rust-lang/crates.io-index`"
    )
    lines = [line.strip() for line in result.stderr.splitlines() if line.strip()]
    errors = [line for line in lines if line.startswith(("error:", "warning:"))]
    if result.returncode == 101 and errors == [exact_not_found]:
        return False
    return None


def rate_limit_bucket(limit: RegistryRateLimit, at: float) -> RateLimitBucket:
    """Start one allowance at the burst the registry states, with nothing yet spent."""

    return RateLimitBucket(limit, float(limit.burst), at, at)


def rate_limit_ready_at(bucket: RateLimitBucket) -> float:
    """The earliest moment the stated allowance and the registry's own answers both permit one."""

    if bucket.level >= 1.0:
        modelled = bucket.updated
    else:
        modelled = bucket.updated + (1.0 - bucket.level) * bucket.limit.refill_seconds
    return max(modelled, bucket.not_before)


def rate_limit_wait(bucket: RateLimitBucket, at: float) -> float:
    """How long an upload at `at` must wait; zero when the allowance already permits it."""

    return max(0.0, rate_limit_ready_at(bucket) - at)


def rate_limit_consume(bucket: RateLimitBucket, at: float) -> RateLimitBucket:
    """Charge one upload at `at`, first refilling the allowance for the time that has passed."""

    elapsed = max(0.0, at - bucket.updated)
    level = min(
        float(bucket.limit.burst), bucket.level + elapsed / bucket.limit.refill_seconds
    )
    return bucket._replace(level=max(0.0, level - 1.0), updated=at)


def rate_limit_restate(bucket: RateLimitBucket, at: float, delay: float) -> RateLimitBucket:
    """Replace the modelled allowance with the deadline the registry itself stated.

    The registry is the authority on its own limit, so its deadline becomes the moment one upload
    is permitted again, and the model resumes from there rather than from this run's optimistic
    start.
    """

    deadline = at + max(0.0, delay)
    return bucket._replace(level=1.0, updated=deadline, not_before=deadline)


def _stated_moment(value: str) -> float | None:
    """Read one RFC 3339 or HTTP-date instant as a Unix timestamp, or nothing at all."""

    text = value.strip()
    try:
        moment = datetime.datetime.fromisoformat(text)
    except ValueError:
        try:
            moment = email.utils.parsedate_to_datetime(text)
        except (TypeError, ValueError):
            return None
    if moment.tzinfo is None:
        moment = moment.replace(tzinfo=datetime.timezone.utc)
    return moment.timestamp()


def _retry_hint_seconds(text: str, now: float) -> float | None:
    """Read the registry's own retry hint: a `Retry-After` header, or its refusal's deadline."""

    header = RETRY_AFTER_HEADER.search(text)
    if header is not None:
        value = header.group(1)
        if re.fullmatch(r"[0-9]+", value):
            return float(value)
        moment = _stated_moment(value)
        if moment is not None:
            return max(0.0, moment - now)
    anchor = RETRY_DEADLINE_ANCHOR.search(text)
    if anchor is not None:
        tail = text[anchor.end() :]
        for pattern in (RFC_3339_MOMENT, HTTP_DATE_MOMENT):
            found = pattern.search(tail)
            if found is None:
                continue
            moment = _stated_moment(found.group(0))
            if moment is not None:
                return max(0.0, moment - now)
    return None


def rate_limit_refusal(text: str, *, now: float) -> RateLimitRefusal | None:
    """Recognise a rate-limit refusal in a Cargo publication report, and the hint it carried.

    A refusal that states no deadline is still a refusal; the caller falls back to the registry's
    stated allowance rather than inventing a delay. Anything that is not a `429` is not a rate
    limit and must not be retried.
    """

    detail = None
    for line in text.splitlines():
        if any(pattern.search(line) for pattern in RATE_LIMIT_SIGNALS):
            detail = line.strip()
            break
    if detail is None:
        return None
    return RateLimitRefusal(_retry_hint_seconds(text, now), detail)


def _resume_note(published: Sequence[str]) -> str:
    """Say what this run did upload, so the operator knows a rerun resumes rather than repeats."""

    accepted = ", ".join(published) if published else "nothing"
    return (
        f"; this run published {accepted} and stopped before any further upload, so a later "
        "invocation resumes from registry visibility"
    )


def publish_frontier(
    frontier: Sequence[str],
    dispatch: Callable[[str], subprocess.CompletedProcess[str]],
    *,
    new_crates: Sequence[str],
    budget_seconds: float,
    monotonic: Callable[[], float] = time.monotonic,
    pause: Callable[[float], None] = time.sleep,
    now: Callable[[], float] = time.time,
    report: Callable[[str], None] = print,
) -> tuple[str, ...]:
    """Upload one frontier at the registry's stated pace, within a finite rate-limit budget.

    No wait here is a fixed delay standing in for the limit. Spacing comes from the allowance the
    registry publishes for the class of upload, and from the deadline the registry states when it
    answers `429`. Exhausting the budget or the retry attempts raises before the next upload is
    dispatched, which is what keeps a stopped run resumable rather than partly repeated.
    """

    if budget_seconds <= 0:
        raise ReleaseError("registry rate-limit budget must be greater than zero")
    names = set(new_crates)
    buckets = {
        NEW_CRATE: rate_limit_bucket(NEW_CRATE_RATE_LIMIT, monotonic()),
        NEW_VERSION: rate_limit_bucket(NEW_VERSION_RATE_LIMIT, monotonic()),
    }
    remaining = float(budget_seconds)
    published: list[str] = []
    for package in frontier:
        kind = NEW_CRATE if package in names else NEW_VERSION
        refusal = None
        for _attempt in range(RATE_LIMIT_RETRY_ATTEMPTS):
            wait = rate_limit_wait(buckets[kind], monotonic())
            if wait > 0.0:
                if wait > remaining:
                    stated = "" if refusal is None else f": {refusal.detail}"
                    raise ReleaseError(
                        f"{package}: the registry's {kind} rate limit needs {wait:g}s before this "
                        f"upload, beyond the {remaining:g}s left in the rate-limit "
                        f"budget{stated}" + _resume_note(published)
                    )
                report(f"{package}: waiting {wait:g}s for the registry's stated {kind} limit")
                pause(wait)
                remaining -= wait
            buckets[kind] = rate_limit_consume(buckets[kind], monotonic())
            result = dispatch(package)
            if result.returncode == 0:
                published.append(package)
                break
            refusal = rate_limit_refusal(result.stdout + result.stderr, now=now())
            if refusal is None:
                raise ReleaseError(
                    f"{package}: publication failed with status {result.returncode}"
                    + _resume_note(published)
                )
            report(f"{package}: {refusal.detail}")
            buckets[kind] = rate_limit_restate(
                buckets[kind],
                monotonic(),
                buckets[kind].limit.refill_seconds
                if refusal.retry_seconds is None
                else refusal.retry_seconds,
            )
        else:
            detail = "" if refusal is None else f": {refusal.detail}"
            raise ReleaseError(
                f"{package}: the registry still refuses publication after "
                f"{RATE_LIMIT_RETRY_ATTEMPTS} rate-limited attempts{detail}"
                + _resume_note(published)
            )
    return tuple(published)


def consumer_manifest(version: str, libraries: Sequence[str]) -> str:
    """A fresh consumer whose only dependency sources are exact registry versions."""

    dependencies = "\n".join(
        f'{name} = {{ version = "={version}", registry = "crates-io" }}'
        for name in sorted(libraries)
    )
    return (
        "[package]\n"
        'name = "sipx-release-consumer"\n'
        'version = "0.0.0"\n'
        'edition = "2024"\n'
        "publish = false\n\n"
        "[dependencies]\n"
        f"{dependencies}\n"
    )


def consumer_install_command(version: str, root: pathlib.Path) -> tuple[str, ...]:
    """Install the exact registry CLI under an isolated temporary prefix."""

    return (
        "cargo",
        "install",
        CLI_PACKAGE,
        "--registry",
        "crates-io",
        "--version",
        f"={version}",
        "--features",
        "opus",
        "--locked",
        "--root",
        str(root),
    )


def _bounded_run(
    command: Sequence[str],
    *,
    cwd: pathlib.Path,
    timeout: float,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        env=env,
    )
    _OWNED_GROUPS[process.pid] = process
    try:
        stdout, stderr = _communicate_owned(process, timeout)
    except TimeoutError as error:
        raise ReleaseError(
            f"command exceeded its {timeout:g}s failure bound: {' '.join(command)}"
        ) from error
    return subprocess.CompletedProcess(
        tuple(command),
        process.returncode,
        stdout.decode("utf-8", errors="replace"),
        stderr.decode("utf-8", errors="replace"),
    )


def _terminate_group(process: subprocess.Popen[object]) -> None:
    """Kill and reap a group while its unreaped leader still reserves the process-group ID."""

    if process.returncode is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    # A failed compiler or build script is not trusted to handle TERM. The group ID is still safe
    # here because `_communicate_owned` deliberately has not reaped its leader.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired as error:
        raise ReleaseError(f"could not reap bounded process group {process.pid}") from error
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            stream.close()


def _communicate_owned(
    process: subprocess.Popen[bytes], timeout: float
) -> tuple[bytes, bytes]:
    """Collect a process without reaping its group leader until every descendant is stopped."""

    if not hasattr(os, "pidfd_open"):
        _terminate_group(process)
        raise ReleaseError("bounded process groups require pidfd support on this release host")
    selector = selectors.DefaultSelector()
    streams: dict[int, tuple[str, object]] = {}
    output = {"stdout": bytearray(), "stderr": bytearray()}
    pidfd = os.pidfd_open(process.pid)
    selector.register(pidfd, selectors.EVENT_READ, "exit")
    for name, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
        if stream is not None:
            descriptor = stream.fileno()
            os.set_blocking(descriptor, False)
            streams[descriptor] = (name, stream)
            selector.register(descriptor, selectors.EVENT_READ, name)
    deadline = time.monotonic() + timeout
    exited = False
    try:
        while not exited or streams:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError
            events = selector.select(remaining)
            if not events:
                raise TimeoutError
            for key, _mask in events:
                if key.data == "exit":
                    exited = True
                    selector.unregister(pidfd)
                    continue
                descriptor = int(key.fd)
                try:
                    chunk = os.read(descriptor, 65_536)
                except BlockingIOError:
                    continue
                if chunk:
                    output[str(key.data)].extend(chunk)
                else:
                    selector.unregister(descriptor)
                    _name, stream = streams.pop(descriptor)
                    stream.close()
    finally:
        selector.close()
        os.close(pidfd)
        owned = _OWNED_GROUPS.pop(process.pid, None)
        if owned is not None:
            _terminate_group(owned)
        for _name, stream in streams.values():
            stream.close()
    return bytes(output["stdout"]), bytes(output["stderr"])


def _cleanup_owned_groups() -> None:
    """Bounded cleanup used by signal handlers before the helper exits."""

    for pid, process in tuple(_OWNED_GROUPS.items()):
        _OWNED_GROUPS.pop(pid, None)
        _terminate_group(process)


def _install_cleanup_handlers() -> None:
    def cleanup(signum: int, _frame: object) -> None:
        _cleanup_owned_groups()
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGINT, cleanup)
    signal.signal(signal.SIGTERM, cleanup)


def _listening_address(process: subprocess.Popen[bytes], timeout: float) -> str:
    """Read a complete JSON line without allowing a partial write to defeat the deadline."""

    if process.stdout is None:
        raise ReleaseError("installed answerer has no stdout pipe")
    descriptor = process.stdout.fileno()
    os.set_blocking(descriptor, False)
    selector = selectors.DefaultSelector()
    selector.register(descriptor, selectors.EVENT_READ)
    deadline = time.monotonic() + timeout
    buffered = bytearray()
    try:
        while b"\n" not in buffered:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not selector.select(max(0.0, remaining)):
                raise ReleaseError(
                    f"installed answerer did not complete its readiness line within {timeout:g}s"
                )
            # One byte prevents consuming part of the terminal report that follows readiness.
            chunk = os.read(descriptor, 1)
            if not chunk:
                raise ReleaseError("installed answerer closed before its readiness report")
            buffered.extend(chunk)
            if len(buffered) > 65_536:
                raise ReleaseError("installed answerer readiness report exceeds 65536 bytes")
    finally:
        selector.close()
    line, _, _remainder = bytes(buffered).partition(b"\n")
    try:
        report = json.loads(line.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"installed answerer emitted invalid JSON: {line!r}") from error
    if report.get("status") != "listening" or not isinstance(report.get("address"), str):
        raise ReleaseError(f"installed answerer did not emit a listening report: {report!r}")
    return str(report["address"])


def _installed_loopback(binary: pathlib.Path, timeout: float) -> None:
    """Place one bounded UDP call through the registry-installed executable."""

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
        start_new_session=True,
    )
    _OWNED_GROUPS[answerer.pid] = answerer
    try:
        address = _listening_address(answerer, min(15.0, timeout))
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
            cwd=binary.parent,
            timeout=timeout,
        )
        if dial.returncode != 0:
            raise ReleaseError(
                f"installed dial failed with {dial.returncode}: {dial.stderr.strip()}"
            )
        try:
            dial_report = json.loads(dial.stdout.splitlines()[-1])
        except (IndexError, json.JSONDecodeError) as error:
            raise ReleaseError(f"installed dial emitted no terminal JSON: {dial.stdout!r}") from error
        if dial_report.get("status") != "answered":
            raise ReleaseError(f"installed dial did not complete a call: {dial_report!r}")
        try:
            answer_stdout, answer_stderr = _communicate_owned(answerer, timeout)
        except TimeoutError as error:
            raise ReleaseError(f"installed answerer exceeded its {timeout:g}s bound") from error
        if answerer.returncode != 0:
            complaint = answer_stderr.decode("utf-8", errors="replace").strip()
            raise ReleaseError(f"installed answerer failed with {answerer.returncode}: {complaint}")
        reports = [json.loads(line) for line in answer_stdout.splitlines() if line]
        if not any(report.get("status") == "answered" for report in reports):
            raise ReleaseError(f"installed answerer emitted no completed-call report: {reports!r}")
    finally:
        owned = _OWNED_GROUPS.pop(answerer.pid, None)
        if owned is not None:
            _terminate_group(owned)


def consumer_environment(
    cargo_home: pathlib.Path, base: dict[str, str] | None = None
) -> dict[str, str]:
    """Isolate consumer reads from source replacements and target paths in user configuration."""

    environment = dict(os.environ if base is None else base)
    for name in tuple(environment):
        if (
            name == "CARGO_HOME"
            or name == "CARGO_TARGET_DIR"
            or name == "CARGO_BUILD_TARGET_DIR"
            or name.startswith("CARGO_SOURCE_")
            or name.startswith("CARGO_REGISTRIES_CRATES_IO_")
        ):
            del environment[name]
    environment["CARGO_HOME"] = str(cargo_home)
    environment["CARGO_REGISTRIES_CRATES_IO_PROTOCOL"] = "sparse"
    return environment


def consumer_lock_problems(
    lock: dict[str, object], libraries: Sequence[str], version: str
) -> list[str]:
    """Require every exact sipx library to come from Cargo's canonical crates.io source."""

    raw_packages = lock.get("package", [])
    packages = {
        str(item.get("name")): item
        for item in raw_packages
        if isinstance(item, dict) and item.get("name") in libraries
    }
    problems = []
    for package in libraries:
        record = packages.get(package, {})
        if record.get("version") != version:
            problems.append(f"consumer lock did not resolve exact package {package}@{version}")
        if record.get("source") != CRATES_IO_LOCK_SOURCE:
            problems.append(f"consumer lock did not resolve {package}@{version} from crates.io")
    return problems


def _archive_evidence(package: Package, archive: pathlib.Path) -> ArchiveEvidence:
    """Hash an archive and read Cargo's bounded VCS record without extracting it."""

    digest = hashlib.sha256()
    with archive.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)

    member_name = f"{package.name}-{package.version}/.cargo_vcs_info.json"
    with tarfile.open(archive, mode="r:gz") as bundle:
        try:
            member = bundle.getmember(member_name)
        except KeyError as error:
            raise ReleaseError(f"{package.name}: packaged archive has no Cargo VCS record") from error
        if not member.isfile() or member.size > 65_536:
            raise ReleaseError(f"{package.name}: packaged Cargo VCS record is not a bounded file")
        stream = bundle.extractfile(member)
        if stream is None:
            raise ReleaseError(f"{package.name}: packaged Cargo VCS record cannot be read")
        with stream:
            record = json.loads(stream.read())
    git = record.get("git") if isinstance(record, dict) else None
    if not isinstance(git, dict) or not isinstance(git.get("sha1"), str):
        raise ReleaseError(f"{package.name}: packaged Cargo VCS record has no Git commit")
    dirty = git.get("dirty", False)
    if "dirty" in git and not isinstance(dirty, bool):
        raise ReleaseError(f"{package.name}: packaged Cargo VCS record has no clean-state fact")
    return ArchiveEvidence(digest.hexdigest(), str(git["sha1"]), dirty)


def _registry_checksums(
    lock: dict[str, object], packages: Sequence[str], version: str
) -> tuple[dict[str, str], list[str]]:
    """Read exact canonical crates.io checksums from a freshly generated Cargo lockfile."""

    raw_packages = lock.get("package", [])
    records = [item for item in raw_packages if isinstance(item, dict)]
    checksums = {}
    problems = []
    for package in packages:
        matches = [
            item
            for item in records
            if item.get("name") == package
            and item.get("version") == version
            and item.get("source") == CRATES_IO_LOCK_SOURCE
        ]
        if len(matches) != 1:
            problems.append(
                f"{package}: registry index did not yield one exact canonical crates.io record"
            )
            continue
        checksum = matches[0].get("checksum")
        if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
            problems.append(f"{package}: crates.io record has no valid SHA-256 checksum")
            continue
        checksums[package] = checksum
    return checksums, problems


def resume_byte_problems(
    available: Sequence[str],
    local: dict[str, ArchiveEvidence],
    registry_checksums: dict[str, str],
    head: str,
) -> list[str]:
    """Refuse a partial release unless visible crates equal this exact clean tagged checkout."""

    problems = []
    for package in sorted(available):
        evidence = local.get(package)
        if evidence is None:
            problems.append(f"{package}: no local tagged archive was produced for resume proof")
            continue
        if evidence.git_sha1 != head:
            problems.append(
                f"{package}: packaged Git commit {evidence.git_sha1} is not tagged commit {head}"
            )
        if evidence.dirty:
            problems.append(f"{package}: packaged archive says the tagged checkout was dirty")
        published = registry_checksums.get(package)
        if published is None:
            problems.append(f"{package}: no crates.io checksum is available for resume proof")
        elif published != evidence.checksum:
            problems.append(
                f"{package}: published bytes differ from the clean tagged archive; "
                "refuse to mix release commits"
            )
    return problems


def verify_resume_bytes(
    packages: Sequence[Package],
    available: Sequence[str],
    version: str,
    *,
    timeout: float,
    workspace_root: pathlib.Path = ROOT,
) -> list[str]:
    """Build local archives and compare visible packages with fresh crates.io index checksums."""

    if not available:
        return []
    head_result = _bounded_run(
        ("git", "rev-parse", "HEAD"), cwd=workspace_root, timeout=min(timeout, 120.0)
    )
    if head_result.returncode != 0:
        raise ReleaseError(head_result.stderr.strip() or "cannot read release commit")
    head = head_result.stdout.strip()
    if re.fullmatch(r"[0-9a-f]{40}", head) is None:
        raise ReleaseError(f"release commit is not a full Git object ID: {head!r}")

    by_name = {package.name: package for package in packages}
    with tempfile.TemporaryDirectory(prefix="sipx-release-resume-") as directory:
        root = pathlib.Path(directory)
        target = root / "package-target"
        command = [
            "cargo",
            "package",
            "--registry",
            "crates-io",
            "--locked",
            "--no-verify",
            "--workspace",
            "--target-dir",
            str(target),
        ]
        for private in sorted(package.name for package in packages if not package.public):
            command.extend(("--exclude", private))
        packaged = _bounded_run(tuple(command), cwd=workspace_root, timeout=timeout)
        if packaged.returncode != 0:
            raise ReleaseError(
                "cannot reproduce clean tagged archives before resuming: "
                + (packaged.stderr.strip() or f"status {packaged.returncode}")
            )

        local = {}
        try:
            for name in sorted(available):
                package = by_name.get(name)
                if package is None:
                    raise ReleaseError(f"{name}: visible package is absent from the workspace")
                archive = target / "package" / f"{name}-{version}.crate"
                if not archive.is_file():
                    raise ReleaseError(f"{name}: Cargo did not reproduce tagged archive {archive}")
                local[name] = _archive_evidence(package, archive)
        except (OSError, tarfile.TarError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ReleaseError(f"cannot read reproduced release archive: {error}") from error

        project = root / "registry-proof"
        (project / "src").mkdir(parents=True)
        (project / "Cargo.toml").write_text(
            consumer_manifest(version, available), encoding="utf-8"
        )
        (project / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        environment = consumer_environment(root / "cargo-home")
        generated = _bounded_run(
            ("cargo", "generate-lockfile"), cwd=project, timeout=timeout, env=environment
        )
        if generated.returncode != 0:
            raise ReleaseError(
                "cannot obtain fresh crates.io checksums before resuming: "
                + (generated.stderr.strip() or f"status {generated.returncode}")
            )
        lock = tomllib.loads((project / "Cargo.lock").read_text(encoding="utf-8"))
        checksums, checksum_problems = _registry_checksums(lock, available, version)
        return checksum_problems + resume_byte_problems(available, local, checksums, head)


def verify_registry_consumer(
    version: str, public_packages: Sequence[str], *, timeout: float
) -> None:
    """Build exact registry libraries, install the CLI, and complete one loopback call."""

    libraries = tuple(package for package in public_packages if package != CLI_PACKAGE)
    with tempfile.TemporaryDirectory(prefix="sipx-registry-consumer-") as directory:
        root = pathlib.Path(directory)
        environment = consumer_environment(root / "cargo-home")
        project = root / "project"
        (project / "src").mkdir(parents=True)
        (project / "Cargo.toml").write_text(
            consumer_manifest(version, libraries), encoding="utf-8"
        )
        (project / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        check = _bounded_run(
            ("cargo", "check"), cwd=project, timeout=timeout, env=environment
        )
        if check.returncode != 0:
            raise ReleaseError(f"exact registry consumer failed: {check.stderr.strip()}")
        lock = tomllib.loads((project / "Cargo.lock").read_text(encoding="utf-8"))
        lock_problems = consumer_lock_problems(lock, libraries, version)
        if lock_problems:
            raise ReleaseError("\n".join(lock_problems))
        install_root = root / "install"
        install = _bounded_run(
            consumer_install_command(version, install_root),
            cwd=root,
            timeout=timeout,
            env=environment,
        )
        if install.returncode != 0:
            raise ReleaseError(f"exact registry CLI install failed: {install.stderr.strip()}")
        _installed_loopback(install_root / "bin" / "sipx", min(timeout, 30.0))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument(
        "--dry-run",
        action="store_true",
        help="verify the staged testkit dependency closure, then run Cargo's locked publication rehearsal",
    )
    modes.add_argument("--publish", action="store_true", help="publish one dependency-ready frontier")
    modes.add_argument(
        "--inspect-dirty-contents",
        action="store_true",
        help="diagnose package listings and local archives without declaring the checkout releasable",
    )
    modes.add_argument(
        "--verify-consumer",
        action="store_true",
        help="after publication, build exact registry crates, install the CLI and run loopback",
    )
    modes.add_argument(
        "--verify-local-package-set",
        action="store_true",
        help="stage testkit's public dependency closure and compile the packaged RTP echo example",
    )
    parser.add_argument(
        "--confirm-publish",
        metavar="TAG",
        help="exact v<version> confirmation; meaningful only with --publish",
    )
    authorizations = parser.add_mutually_exclusive_group()
    authorizations.add_argument(
        "--authorize-ci-publish",
        metavar="TAG@SHA",
        help="exact tag and full commit authorization required for GitHub Actions publication",
    )
    authorizations.add_argument(
        "--authorize-ci-recovery",
        metavar="TAG@SHA@FAILED_RUN_ID",
        help="exact release commit and failed run authorization for main-workflow recovery",
    )
    parser.add_argument(
        "--release-root",
        metavar="PATH",
        help="exact release checkout used for every Cargo, Git and publication operation",
    )
    parser.add_argument(
        "--registry-wait-seconds",
        type=float,
        default=120.0,
        help="finite visibility bound after an upload (publish mode only; default: 120)",
    )
    parser.add_argument(
        "--registry-retry-budget-seconds",
        type=float,
        default=1800.0,
        help=(
            "finite total wait for the registry's stated publication limits and its 429 deadlines "
            "during one publication (publish mode only; default: 1800)"
        ),
    )
    parser.add_argument(
        "--consumer-timeout-seconds",
        type=float,
        default=900.0,
        help="finite bound for each consumer build/install command (default: 900)",
    )
    parser.add_argument(
        "--command-timeout-seconds",
        type=float,
        default=1800.0,
        help="finite bound for package and publish commands (default: 1800)",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    _install_cleanup_handlers()
    args = _parser().parse_args(argv)
    mode = (
        "publish"
        if args.publish
        else "dry-run"
        if args.dry_run
        else "inspect-contents"
        if args.inspect_dirty_contents
        else "verify-consumer"
        if args.verify_consumer
        else "verify-local-package-set"
        if args.verify_local_package_set
        else "check"
    )
    if args.confirm_publish is not None and mode != "publish":
        print("--confirm-publish is valid only with --publish", file=sys.stderr)
        return 1
    if args.authorize_ci_publish is not None and mode != "publish":
        print("--authorize-ci-publish is valid only with --publish", file=sys.stderr)
        return 1
    if args.authorize_ci_recovery is not None and mode != "publish":
        print("--authorize-ci-recovery is valid only with --publish", file=sys.stderr)
        return 1
    if args.authorize_ci_recovery is not None and args.release_root is None:
        print("--authorize-ci-recovery requires an explicit --release-root", file=sys.stderr)
        return 1
    if args.registry_wait_seconds <= 0:
        print("--registry-wait-seconds must be greater than zero", file=sys.stderr)
        return 1
    if args.registry_retry_budget_seconds <= 0:
        print("--registry-retry-budget-seconds must be greater than zero", file=sys.stderr)
        return 1
    if args.consumer_timeout_seconds <= 0:
        print("--consumer-timeout-seconds must be greater than zero", file=sys.stderr)
        return 1
    if args.command_timeout_seconds <= 0:
        print("--command-timeout-seconds must be greater than zero", file=sys.stderr)
        return 1

    try:
        release_root = (
            pathlib.Path(args.release_root).resolve() if args.release_root is not None else ROOT
        )
        root_manifest = tomllib.loads((release_root / "Cargo.toml").read_text(encoding="utf-8"))
        workspace_package = root_manifest["workspace"]["package"]
        version = str(workspace_package["version"])
        license_expression = str(workspace_package["license"])
        metadata = _metadata(release_root)
        records = metadata.get("packages")
        if not isinstance(records, list):
            raise ReleaseError("cargo metadata has no package list")
        packages = package_records(records, version, release_root)
        problems = graph_problems(packages, version, release_root)
        problems.extend(metadata_problems(packages, release_root, license_expression))
        if mode == "inspect-contents":
            content_problems = inspect_contents(packages, release_root)
            problems.extend(content_problems)
            public_count = sum(package.public for package in packages)
            print(
                f"inspected Cargo listings and normalized local archives for {public_count} "
                "public packages"
            )
            print("diagnostic only: dirty content inspection does not validate a release checkout")
            if problems:
                raise ReleaseError("\n".join(f"- {problem}" for problem in problems))
            return 0
        if mode == "verify-local-package-set":
            if problems:
                raise ReleaseError("\n".join(f"- {problem}" for problem in problems))
            verify_local_rtp_echo_package_set(
                packages,
                version,
                package_timeout=args.command_timeout_seconds,
                consumer_timeout=args.consumer_timeout_seconds,
                workspace_root=release_root,
            )
            print(
                "exact staged sipx-testkit dependency closure and RTP echo example passed "
                "the clean-consumer proof"
            )
            print("diagnostic only: local package-set proof does not claim registry visibility")
            return 0
        dirty, tags, annotated_tags = _checkout(release_root)
        ci = bool(os.environ.get("CI") or os.environ.get("GITHUB_ACTIONS"))
        head_sha = (
            _head_commit(release_root)
            if mode == "publish"
            and (ci or args.authorize_ci_publish or args.authorize_ci_recovery)
            else None
        )
        controller_sha = _head_commit(ROOT) if ci and args.authorize_ci_recovery else None
        problems.extend(
            checkout_problems(
                mode,
                version,
                dirty=dirty,
                tags=tags,
                annotated_tags=annotated_tags,
                confirmation=args.confirm_publish,
                ci=ci,
                head_sha=head_sha,
                ci_authorization=args.authorize_ci_publish,
                ci_recovery_authorization=args.authorize_ci_recovery,
                controller_sha=controller_sha,
                ci_environment=os.environ,
            )
        )
        if problems:
            raise ReleaseError("\n".join(f"- {problem}" for problem in problems))

        order = publication_order(packages)
        print(f"workspace {version}: {len(order)} public packages")
        print("dependency order: " + ", ".join(order))
        if mode == "dry-run":
            verify_local_rtp_echo_package_set(
                packages,
                version,
                package_timeout=args.command_timeout_seconds,
                consumer_timeout=args.consumer_timeout_seconds,
                workspace_root=release_root,
            )
            print(
                "exact staged sipx-testkit dependency closure and RTP echo example passed "
                "the clean-consumer proof"
            )
        if mode == "verify-consumer":
            visibility = poll_registry_visibility(
                order,
                lambda package, remaining: _registry_available(
                    package, version, min(15.0, remaining), release_root
                ),
                timeout=args.registry_wait_seconds,
                interval=2.0,
            )
            if visibility.missing:
                raise ReleaseError(
                    "consumer verification requires every exact package to be registry-visible; "
                    f"missing after {args.registry_wait_seconds:g}s: "
                    + ", ".join(visibility.missing)
                )
            resume_problems = verify_resume_bytes(
                packages,
                visibility.available,
                version,
                timeout=args.command_timeout_seconds,
                workspace_root=release_root,
            )
            if resume_problems:
                raise ReleaseError(
                    "registry packages do not match the immutable release bytes:\n"
                    + "\n".join(f"- {problem}" for problem in resume_problems)
                )
            verify_registry_consumer(
                version, order, timeout=args.consumer_timeout_seconds
            )
            print("exact registry libraries and installed CLI completed the loopback proof")
            return 0
        if mode == "publish":
            available = {
                name for name in order if _registry_available(name, version, 15.0, release_root)
            }
            visibility_problems = recovery_visibility_problems(
                args.authorize_ci_recovery, tuple(sorted(available))
            )
            if visibility_problems:
                raise ReleaseError("\n".join(f"- {problem}" for problem in visibility_problems))
            resume_problems = verify_resume_bytes(
                packages,
                tuple(sorted(available)),
                version,
                timeout=args.command_timeout_seconds,
                workspace_root=release_root,
            )
            if resume_problems:
                raise ReleaseError(
                    "publication does not match the immutable release bytes:\n"
                    + "\n".join(f"- {problem}" for problem in resume_problems)
                )
            frontier = ready_frontier(packages, available)
            if not frontier:
                if len(available) == len(order):
                    print("all public packages are already registry-visible")
                    return 0
                missing = sorted(set(order) - available)
                raise ReleaseError(
                    "no package is dependency-ready; registry visibility is incomplete for: "
                    + ", ".join(missing)
                )
            uploads = dict(zip(frontier, commands_for(mode, frontier), strict=True))
            new_crates = []
            for name in frontier:
                known = _registry_name_exists(name, 15.0, release_root)
                if known is None:
                    print(
                        f"{name}: the registry gave no readable name answer; pacing it under the "
                        "stated new-crate limit"
                    )
                if not known:
                    new_crates.append(name)
            print(
                f"registry pacing: {len(new_crates)} new crate name(s), "
                f"{len(frontier) - len(new_crates)} existing name(s)"
            )
            publish_frontier(
                frontier,
                lambda package: _dispatch(
                    uploads[package], release_root, args.command_timeout_seconds
                ),
                new_crates=new_crates,
                budget_seconds=args.registry_retry_budget_seconds,
            )
        else:
            excluded = tuple(package.name for package in packages if not package.public)
            for command in commands_for(mode, order, excluded=excluded):
                result = _dispatch(command, release_root, args.command_timeout_seconds)
                if result.returncode != 0:
                    raise ReleaseError(
                        f"command failed with status {result.returncode}: {' '.join(command)}"
                    )
        if mode == "publish":
            visibility = poll_registry_visibility(
                frontier,
                lambda package, remaining: _registry_available(
                    package, version, min(15.0, remaining), release_root
                ),
                timeout=args.registry_wait_seconds,
                interval=2.0,
            )
            if visibility.missing:
                raise ReleaseError(
                    "published packages were not registry-visible within "
                    f"{args.registry_wait_seconds:g}s: {', '.join(visibility.missing)}; "
                    "verify visibility before resuming publication"
                )
            print("published frontier is registry-visible; rerun to advance")
        elif mode == "dry-run":
            print("all public packages passed cargo publish --dry-run --locked")
        return 0
    except (OSError, KeyError, json.JSONDecodeError, tomllib.TOMLDecodeError, ReleaseError) as error:
        print(f"release rehearsal refused:\n{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
