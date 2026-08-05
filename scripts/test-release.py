#!/usr/bin/env python3
"""Adversarial tests for the registry release rehearsal (A-11).

Authority tests use fabricated Cargo metadata and checkout state. The local package-set case alone
executes bounded `cargo package` and clean-consumer commands against the workspace. No test has
registry credentials and no test runs `cargo publish`; write-capable dispatch stays behind a
recording runner.
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import pathlib
import select
import signal
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("sipx_release", ROOT / "scripts" / "release.py")
assert SPEC is not None and SPEC.loader is not None
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def package(
    name: str,
    *,
    version: str = "1.0.0-beta.1",
    publish: list[str] | None = None,
    dependencies: tuple[dict[str, object], ...] = (),
    manifest_path: str | None = None,
    readme: str | None = "README.md",
    license_: str = "MIT OR Apache-2.0",
) -> dict[str, object]:
    return {
        "name": name,
        "version": version,
        "publish": publish,
        "dependencies": list(dependencies),
        "manifest_path": manifest_path or f"/work/crates/{name}/Cargo.toml",
        "readme": readme,
        "license": license_,
    }


def dependency(
    name: str,
    *,
    req: str = "^1.0.0-beta.1",
    path: str | None = None,
    kind: str | None = None,
    source: str | None = None,
) -> dict[str, object]:
    return {"name": name, "req": req, "path": path, "kind": kind, "source": source}


def github_publish_environment(
    tag: str, sha: str, *, event: str = "push"
) -> dict[str, str]:
    """One complete GitHub-provided context for the exact release tag and commit."""

    return {
        "CI": "true",
        "GITHUB_ACTIONS": "true",
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_REPOSITORY": "codewandler/sipx",
        "GITHUB_EVENT_NAME": event,
        "GITHUB_REF": f"refs/tags/{tag}",
        "GITHUB_REF_TYPE": "tag",
        "GITHUB_REF_NAME": tag,
        "GITHUB_SHA": sha,
        "GITHUB_WORKFLOW_SHA": sha,
        "GITHUB_WORKFLOW_REF": f"codewandler/sipx/.github/workflows/crates-io.yml@refs/tags/{tag}",
        "GITHUB_RUN_ID": "123456",
        "GITHUB_RUN_ATTEMPT": "1",
        "CARGO_REGISTRY_TOKEN": "fixture-secret-never-used",
    }


def github_recovery_environment(
    controller_sha: str, *, failed_run_id: str = "654321"
) -> dict[str, str]:
    """One complete GitHub context for the main-branch recovery controller."""

    return {
        "CI": "true",
        "GITHUB_ACTIONS": "true",
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_REPOSITORY": "codewandler/sipx",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_REF_TYPE": "branch",
        "GITHUB_REF_NAME": "main",
        "GITHUB_SHA": controller_sha,
        "GITHUB_WORKFLOW_SHA": controller_sha,
        "GITHUB_WORKFLOW_REF": (
            "codewandler/sipx/.github/workflows/crates-io-resume.yml@refs/heads/main"
        ),
        "GITHUB_RUN_ID": "123456",
        "GITHUB_RUN_ATTEMPT": "1",
        "CARGO_REGISTRY_TOKEN": "fixture-secret-never-used",
        "SIPX_FAILED_RELEASE_RUN_ID": failed_run_id,
    }


class ThePublicPackageGraph(unittest.TestCase):
    def test_dependencies_are_before_dependants_with_a_stable_name_tiebreak(self) -> None:
        packages = release.package_records(
            [
                package("sipx-cli", dependencies=(dependency("sipx-call", path="/work/crates/sipx-call"),)),
                package("sipx-zed"),
                package("sipx-call", dependencies=(dependency("sipx-core", path="/work/crates/sipx-core"),)),
                package("sipx-core"),
                package("sipx-testkit", publish=[]),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        self.assertEqual(
            ("sipx-core", "sipx-call", "sipx-cli", "sipx-zed"),
            release.publication_order(packages),
        )

    def test_a_cycle_is_refused_instead_of_becoming_an_arbitrary_order(self) -> None:
        packages = release.package_records(
            [
                package("sipx-a", dependencies=(dependency("sipx-b", path="/work/crates/sipx-b"),)),
                package("sipx-b", dependencies=(dependency("sipx-a", path="/work/crates/sipx-a"),)),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        with self.assertRaisesRegex(release.ReleaseError, "cycle"):
            release.publication_order(packages)

    def test_an_unpublished_normal_dependency_is_a_registry_escape(self) -> None:
        packages = release.package_records(
            [
                package(
                    "sipx-cli",
                    dependencies=(dependency("sipx-testkit", path="/work/crates/sipx-testkit"),),
                ),
                package("sipx-testkit", publish=[]),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        problems = release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work"))
        self.assertTrue(any("sipx-cli" in p and "sipx-testkit" in p and "does not publish" in p for p in problems))

    def test_a_path_only_dev_dependency_is_dropped_from_the_archive_graph(self) -> None:
        packages = release.package_records(
            [
                package(
                    "sipx-core",
                    dependencies=(
                        dependency(
                            "sipx-testkit",
                            req="*",
                            path="/work/crates/sipx-testkit",
                            kind="dev",
                        ),
                    ),
                ),
                package("sipx-testkit", publish=[]),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        self.assertEqual([], release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work")))

    def test_a_versioned_dev_dependency_on_unpublished_support_is_refused(self) -> None:
        packages = release.package_records(
            [
                package(
                    "sipx-core",
                    dependencies=(
                        dependency("sipx-testkit", path="/work/crates/sipx-testkit", kind="dev"),
                    ),
                ),
                package("sipx-testkit", publish=[]),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        problems = release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work"))
        self.assertTrue(any("versioned dev-dependency" in problem for problem in problems))


class TheManifestBoundary(unittest.TestCase):
    def test_real_public_manifests_keep_workspace_testkit_path_only(self) -> None:
        for manifest_path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
            if manifest["package"].get("publish", True) is False:
                continue
            testkit = manifest.get("dev-dependencies", {}).get("sipx-testkit")
            if testkit is None:
                continue
            self.assertEqual(
                {"path": "../sipx-testkit"},
                testkit,
                f"{manifest_path}: versioned unpublished dev support survives normalization",
            )

    def test_package_and_internal_requirement_versions_must_match(self) -> None:
        packages = release.package_records(
            [
                package("sipx-core", version="1.0.0-beta.2"),
                package(
                    "sipx-call",
                    dependencies=(dependency("sipx-core", req="^1.0.0-beta.2", path="/work/crates/sipx-core"),),
                ),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        problems = release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work"))
        self.assertTrue(any("sipx-core" in p and "package version" in p for p in problems))
        self.assertTrue(any("sipx-call" in p and "requirement" in p for p in problems))

    def test_git_and_outside_or_versionless_paths_are_refused(self) -> None:
        packages = release.package_records(
            [
                package(
                    "sipx-core",
                    dependencies=(
                        dependency("remote", source="git+https://invalid.example/repo"),
                        dependency("outside", req="*", path="/elsewhere/outside"),
                    ),
                ),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        problems = release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work"))
        self.assertTrue(any("Git dependency" in p for p in problems))
        self.assertTrue(any("outside the workspace" in p for p in problems))

    def test_an_in_tree_path_that_is_not_a_workspace_package_is_refused(self) -> None:
        packages = release.package_records(
            [
                package(
                    "sipx-core",
                    dependencies=(
                        dependency("hidden", path="/work/vendor/hidden"),
                    ),
                ),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        problems = release.graph_problems(packages, "1.0.0-beta.1", pathlib.Path("/work"))
        self.assertTrue(any("hidden" in p and "not a workspace package" in p for p in problems))

    def test_every_public_package_needs_a_real_readme_and_license_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            crate = root / "crates" / "sipx-core"
            crate.mkdir(parents=True)
            manifest = crate / "Cargo.toml"
            manifest.write_text("[package]\nname='sipx-core'\n", encoding="utf-8")
            packages = release.package_records(
                [package("sipx-core", manifest_path=str(manifest), readme="README.md")],
                "1.0.0-beta.1",
                root,
            )
            problems = release.metadata_problems(packages, root, "MIT OR Apache-2.0")
            self.assertTrue(any("README" in p for p in problems))
            self.assertTrue(any("LICENSE-MIT" in p for p in problems))
            self.assertTrue(any("LICENSE-APACHE" in p for p in problems))

    def test_archive_listing_requires_manifest_readme_and_confined_paths(self) -> None:
        package_record = release.Package(
            name="sipx-core",
            version="1.0.0-beta.1",
            public=True,
            dependencies=(),
            manifest=pathlib.Path("/work/crates/sipx-core/Cargo.toml"),
            readme=pathlib.Path("/work/crates/sipx-core/README.md"),
            license="MIT OR Apache-2.0",
        )
        problems = release.archive_listing_problems(
            package_record,
            ("Cargo.toml", "Cargo.toml.orig", "../workspace-secret", "src/lib.rs"),
        )
        self.assertTrue(any("README" in problem for problem in problems))
        self.assertTrue(any("escapes" in problem for problem in problems))

    def test_normalized_manifest_retains_release_metadata_and_no_local_sources(self) -> None:
        package_record = release.Package(
            name="sipx-call",
            version="1.0.0-beta.1",
            public=True,
            dependencies=(),
            manifest=pathlib.Path("/work/crates/sipx-call/Cargo.toml"),
            readme=pathlib.Path("/work/crates/sipx-call/README.md"),
            license="MIT OR Apache-2.0",
        )
        manifest = """
[package]
name = "sipx-call"
version = "1.0.0-beta.1"
license = "MIT OR Apache-2.0"
readme = "README.md"

[dependencies.sipx-sip]
version = "1.0.0-beta.1"
path = "../sipx-sip"

[build-dependencies.generator]
git = "https://invalid.example/generator"
"""
        problems = release.normalized_manifest_problems(
            package_record,
            manifest,
            workspace_packages={"sipx-call", "sipx-sip"},
            public_packages={"sipx-call", "sipx-sip"},
        )
        self.assertTrue(any("path" in problem for problem in problems))
        self.assertTrue(any("Git" in problem for problem in problems))

    def test_normalized_manifest_refuses_an_unpublished_workspace_dependency(self) -> None:
        package_record = release.Package(
            name="sipx-cli",
            version="1.0.0-beta.1",
            public=True,
            dependencies=(),
            manifest=pathlib.Path("/work/crates/sipx-cli/Cargo.toml"),
            readme=pathlib.Path("/work/crates/sipx-cli/README.md"),
            license="MIT OR Apache-2.0",
        )
        manifest = """
[package]
name = "sipx-cli"
version = "1.0.0-beta.1"
license = "MIT OR Apache-2.0"
readme = "README.md"

[dependencies.sipx-testkit]
version = "1.0.0-beta.1"
"""
        problems = release.normalized_manifest_problems(
            package_record,
            manifest,
            workspace_packages={"sipx-cli", "sipx-testkit"},
            public_packages={"sipx-cli"},
        )
        self.assertTrue(any("sipx-testkit" in problem and "unpublished" in problem for problem in problems))

    def test_a_well_formed_archive_boundary_is_accepted(self) -> None:
        package_record = release.Package(
            name="sipx-core",
            version="1.0.0-beta.1",
            public=True,
            dependencies=(),
            manifest=pathlib.Path("/work/crates/sipx-core/Cargo.toml"),
            readme=pathlib.Path("/work/crates/sipx-core/README.md"),
            license="MIT OR Apache-2.0",
        )
        listing = ("Cargo.toml", "Cargo.toml.orig", "README.md", "src/lib.rs")
        manifest = """
[package]
name = "sipx-core"
version = "1.0.0-beta.1"
license = "MIT OR Apache-2.0"
readme = "README.md"
"""
        self.assertEqual([], release.archive_listing_problems(package_record, listing))
        self.assertEqual(
            [],
            release.normalized_manifest_problems(
                package_record,
                manifest,
                workspace_packages={"sipx-core"},
                public_packages={"sipx-core"},
            ),
        )


class TheLocalPackageSetProof(unittest.TestCase):
    def test_packaged_consumer_example_is_derived_from_surface_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory)
            (source / "examples").mkdir()
            (source / "Cargo.toml").write_text(
                """[package]
name = "sipx-testkit"
[package.metadata.sipx-supported-test-surface]
example = "public_harness"
""",
                encoding="utf-8",
            )
            example = source / "examples" / "public_harness.rs"
            example.write_text("use sipx_testkit::call::CallHarness;\n", encoding="utf-8")
            self.assertEqual(example, release.supported_test_surface_example(source))
            example.unlink()
            with self.assertRaisesRegex(release.ReleaseError, "omits examples/public_harness.rs"):
                release.supported_test_surface_example(source)

    def test_package_set_is_the_dependency_ordered_testkit_closure(self) -> None:
        packages = release.package_records(
            [
                package("sipx-testkit", dependencies=(dependency("sipx-transport"),)),
                package("sipx-transport", dependencies=(dependency("sipx-sip"),)),
                package("sipx-sip"),
                package("sipx-unrelated"),
            ],
            "1.0.0-beta.4",
            pathlib.Path("/work"),
        )
        self.assertEqual(
            ("sipx-sip", "sipx-transport", "sipx-testkit"),
            release.local_package_set_members(packages),
        )

    def test_consumer_uses_every_exact_staged_package_source(self) -> None:
        manifest = tomllib.loads(
            release.local_package_consumer_manifest(
                "1.0.0-beta.4",
                {
                    "sipx-sip": pathlib.Path("/staged/sipx-sip-1.0.0-beta.4"),
                    "sipx-testkit": pathlib.Path("/staged/sipx-testkit-1.0.0-beta.4"),
                    "sipx-transport": pathlib.Path("/staged/sipx-transport-1.0.0-beta.4"),
                },
            )
        )
        self.assertEqual(
            {
                "version": "=1.0.0-beta.4",
                "path": "/staged/sipx-testkit-1.0.0-beta.4",
            },
            manifest["dependencies"]["sipx-testkit"],
        )
        self.assertEqual(
            {
                "version": "=1.0.0-beta.4",
                "path": "/staged/sipx-transport-1.0.0-beta.4",
            },
            manifest["patch"]["crates-io"]["sipx-transport"],
        )
        self.assertEqual(
            {
                "version": "=1.0.0-beta.4",
                "path": "/staged/sipx-sip-1.0.0-beta.4",
            },
            manifest["patch"]["crates-io"]["sipx-sip"],
        )

    def test_lock_must_resolve_every_package_set_member_from_staged_paths(self) -> None:
        good = {
            "package": [
                {"name": "sipx-sip", "version": "1.0.0-beta.4"},
                {"name": "sipx-testkit", "version": "1.0.0-beta.4"},
                {"name": "sipx-transport", "version": "1.0.0-beta.4"},
            ]
        }
        members = ("sipx-sip", "sipx-transport", "sipx-testkit")
        self.assertEqual([], release.local_package_lock_problems(good, "1.0.0-beta.4", members))
        registry = {
            "package": [
                {
                    "name": "sipx-sip",
                    "version": "1.0.0-beta.4",
                    "source": release.CRATES_IO_LOCK_SOURCE,
                },
                {
                    "name": "sipx-testkit",
                    "version": "1.0.0-beta.4",
                    "source": release.CRATES_IO_LOCK_SOURCE,
                },
                {"name": "sipx-transport", "version": "1.0.0-beta.4"},
            ]
        }
        self.assertTrue(
            any(
                "registry instead of staged bytes" in problem
                for problem in release.local_package_lock_problems(
                    registry, "1.0.0-beta.4", members
                )
            )
        )

    def test_staged_archive_extraction_refuses_escape_and_links(self) -> None:
        for name, member in (
            ("escape", tarfile.TarInfo("sipx-testkit-1.0.0-beta.4/../secret")),
            ("link", tarfile.TarInfo("sipx-testkit-1.0.0-beta.4/link")),
        ):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                archive = root / "sipx-testkit-1.0.0-beta.4.crate"
                payload = b"secret"
                if name == "link":
                    member.type = tarfile.SYMTYPE
                    member.linkname = "/outside"
                else:
                    member.size = len(payload)
                with tarfile.open(archive, mode="w:gz") as bundle:
                    bundle.addfile(member, None if name == "link" else io.BytesIO(payload))
                with self.assertRaisesRegex(release.ReleaseError, "escapes|regular file"):
                    release._extract_package_source(archive, root / "staged")

    def test_real_archives_compile_the_example_in_an_isolated_consumer(self) -> None:
        """Execute the complete dependency-closure proof under its owned finite command bounds."""

        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
        version = str(workspace["package"]["version"])
        metadata = release._metadata(ROOT)
        records = metadata.get("packages")
        self.assertIsInstance(records, list)
        packages = release.package_records(records, version, ROOT)
        release.verify_local_rtp_echo_package_set(
            packages,
            version,
            package_timeout=300.0,
            consumer_timeout=900.0,
            workspace_root=ROOT,
        )

    def test_live_release_graph_names_the_current_workspace_version(self) -> None:
        """Keep the protected rehearsal from being the first live-graph version check."""

        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
        version = str(workspace["package"]["version"])
        metadata = release._metadata(ROOT)
        records = metadata.get("packages")
        self.assertIsInstance(records, list)
        packages = release.package_records(records, version, ROOT)
        self.assertEqual([], release.graph_problems(packages, version, ROOT))


class ThePackagedVcsEvidence(unittest.TestCase):
    def package(self) -> release.Package:
        return release.Package(
            name="sipx-core",
            version="1.0.0-beta.1",
            public=True,
            dependencies=(),
            manifest=pathlib.Path("/work/crates/sipx-core/Cargo.toml"),
            readme=pathlib.Path("/work/crates/sipx-core/README.md"),
            license="MIT OR Apache-2.0",
        )

    def archive(self, root: pathlib.Path, *, dirty: object = ...) -> pathlib.Path:
        package_record = self.package()
        archive = root / f"{package_record.name}-{package_record.version}.crate"
        git: dict[str, object] = {"sha1": "a" * 40}
        if dirty is not ...:
            git["dirty"] = dirty
        payload = json.dumps({"git": git}).encode("utf-8")
        member = tarfile.TarInfo(
            f"{package_record.name}-{package_record.version}/.cargo_vcs_info.json"
        )
        member.size = len(payload)
        with tarfile.open(archive, mode="w:gz") as bundle:
            bundle.addfile(member, io.BytesIO(payload))
        return archive

    def test_cargo_omitting_dirty_is_clean_archive_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = release._archive_evidence(
                self.package(), self.archive(pathlib.Path(directory))
            )
        self.assertFalse(evidence.dirty)
        self.assertEqual("a" * 40, evidence.git_sha1)

    def test_present_non_boolean_dirty_is_refused(self) -> None:
        for dirty in (None, 0, "false", []):
            with self.subTest(dirty=dirty), tempfile.TemporaryDirectory() as directory:
                with self.assertRaisesRegex(release.ReleaseError, "clean-state fact"):
                    release._archive_evidence(
                        self.package(), self.archive(pathlib.Path(directory), dirty=dirty)
                    )

    def test_present_true_dirty_remains_dirty_archive_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = release._archive_evidence(
                self.package(), self.archive(pathlib.Path(directory), dirty=True)
            )
        self.assertTrue(evidence.dirty)


class ThePublicationBoundary(unittest.TestCase):
    def setUp(self) -> None:
        # GitHub itself sets CI=true. Most tests exercise local authority, so make that path
        # explicit rather than allowing the machine running this suite to select semantics.
        patcher = mock.patch.dict(os.environ, {"CI": "", "GITHUB_ACTIONS": ""})
        patcher.start()
        self.addCleanup(patcher.stop)

    def main_metadata(self, *, two_public: bool = False) -> dict[str, object]:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
        version = workspace["package"]["version"]
        crate = ROOT / "crates" / "sipx-sip"
        records = [
            package(
                "sipx-sip",
                version=version,
                manifest_path=str(crate / "Cargo.toml"),
                readme=str(crate / "README.md"),
            )
        ]
        if two_public:
            second = ROOT / "crates" / "sipx-sdp"
            records.append(
                package(
                    "sipx-sdp",
                    version=version,
                    manifest_path=str(second / "Cargo.toml"),
                    readme=str(second / "README.md"),
                )
            )
        return {
            "packages": records
        }

    def test_main_dry_run_dispatch_is_bounded_and_names_crates_io(self) -> None:
        calls: list[tuple[tuple[str, ...], float]] = []

        def bounded(command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None):
            del cwd, env
            calls.append((tuple(command), timeout))
            return subprocess.CompletedProcess(command, 0, "", "")

        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (), ())),
            mock.patch.object(release, "_bounded_run", side_effect=bounded),
            mock.patch.object(release, "verify_local_rtp_echo_package_set") as package_set,
        ):
            self.assertEqual(0, release.main(("--dry-run", "--command-timeout-seconds", "7")))
        package_set.assert_called_once()
        self.assertEqual(7.0, package_set.call_args.kwargs["package_timeout"])
        self.assertEqual(900.0, package_set.call_args.kwargs["consumer_timeout"])
        self.assertEqual(ROOT, package_set.call_args.kwargs["workspace_root"])
        self.assertEqual(1, len(calls))
        command, timeout = calls[0]
        self.assertEqual(7.0, timeout)
        self.assertIn("--dry-run", command)
        self.assertEqual("crates-io", command[command.index("--registry") + 1])

    def test_main_publish_consumes_the_annotated_checkout_and_bounded_dispatch(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        calls = []
        visible = iter((False, True))

        def bounded(command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None):
            del cwd, env
            calls.append((tuple(command), timeout))
            return subprocess.CompletedProcess(command, 0, "", "")

        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_registry_available", side_effect=lambda *_args: next(visible)),
            mock.patch.object(release, "_bounded_run", side_effect=bounded),
        ):
            self.assertEqual(
                0,
                release.main(
                    (
                        "--publish",
                        "--confirm-publish",
                        tag,
                        "--command-timeout-seconds",
                        "9",
                    )
                ),
            )
        self.assertEqual(1, len(calls))
        self.assertEqual(9.0, calls[0][1])
        self.assertEqual("crates-io", calls[0][0][calls[0][0].index("--registry") + 1])

    def test_main_authorized_tag_push_retains_the_bounded_frontier_dispatch(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        sha = "a" * 40
        calls = []
        visible = iter((False, True))

        def bounded(command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None):
            del cwd, env
            calls.append((tuple(command), timeout))
            return subprocess.CompletedProcess(command, 0, "", "")

        with (
            mock.patch.dict(os.environ, github_publish_environment(tag, sha)),
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_head_commit", return_value=sha),
            mock.patch.object(
                release, "_registry_available", side_effect=lambda *_args: next(visible)
            ),
            mock.patch.object(release, "_bounded_run", side_effect=bounded),
        ):
            self.assertEqual(
                0,
                release.main(
                    (
                        "--publish",
                        "--confirm-publish",
                        tag,
                        "--authorize-ci-publish",
                        f"{tag}@{sha}",
                        "--command-timeout-seconds",
                        "11",
                    )
                ),
            )
        self.assertEqual(1, len(calls))
        self.assertEqual(11.0, calls[0][1])
        self.assertEqual("crates-io", calls[0][0][calls[0][0].index("--registry") + 1])

    def test_controller_uses_an_explicit_release_root_without_writing_its_script_there(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        calls: list[tuple[tuple[str, ...], pathlib.Path]] = []
        visible = iter((False, True))
        with tempfile.TemporaryDirectory() as directory:
            release_root = pathlib.Path(directory).resolve()
            crate = release_root / "crates" / "sipx-sip"
            crate.mkdir(parents=True)
            (release_root / "Cargo.toml").write_text(
                "[workspace]\nmembers = [\"crates/*\"]\n"
                f"[workspace.package]\nversion = \"{version}\"\n"
                'license = "MIT OR Apache-2.0"\n',
                encoding="utf-8",
            )
            for filename in ("LICENSE-MIT", "LICENSE-APACHE"):
                (release_root / filename).write_text("fixture\n", encoding="utf-8")
            (crate / "Cargo.toml").write_text(
                "[package]\nname = \"sipx-sip\"\n"
                f"version = \"{version}\"\n",
                encoding="utf-8",
            )
            (crate / "README.md").write_text("fixture\n", encoding="utf-8")
            metadata = {
                "packages": [
                    package(
                        "sipx-sip",
                        version=version,
                        manifest_path=str(crate / "Cargo.toml"),
                        readme=str(crate / "README.md"),
                    )
                ]
            }

            def bounded(
                command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None
            ):
                del timeout, env
                calls.append((tuple(command), cwd))
                return subprocess.CompletedProcess(command, 0, "", "")

            with (
                mock.patch.object(release, "_install_cleanup_handlers"),
                mock.patch.object(release, "_metadata", return_value=metadata) as metadata_call,
                mock.patch.object(
                    release, "_checkout", return_value=(False, (tag,), (tag,))
                ) as checkout_call,
                mock.patch.object(
                    release, "_registry_available", side_effect=lambda *_args: next(visible)
                ),
                mock.patch.object(release, "_bounded_run", side_effect=bounded),
            ):
                self.assertEqual(
                    0,
                    release.main(
                        (
                            "--publish",
                            "--confirm-publish",
                            tag,
                            "--release-root",
                            str(release_root),
                        )
                    ),
                )
            metadata_call.assert_called_once_with(release_root)
            checkout_call.assert_called_once_with(release_root)
            self.assertEqual([(calls[0][0], release_root)], calls)
            self.assertFalse((release_root / "scripts" / "release.py").exists())

    def test_main_generic_ci_refuses_before_registry_probe_or_upload(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        sha = "a" * 40
        with (
            mock.patch.dict(os.environ, {"CI": "true", "GITHUB_ACTIONS": ""}),
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_head_commit", return_value=sha),
            mock.patch.object(release, "_registry_available") as registry,
            mock.patch.object(release, "_bounded_run") as bounded,
        ):
            self.assertEqual(1, release.main(("--publish", "--confirm-publish", tag)))
        registry.assert_not_called()
        bounded.assert_not_called()

    def test_main_partial_publish_checksum_mismatch_dispatches_no_upload(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        calls = []
        visible = iter((True, False))

        def bounded(command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None):
            del cwd, timeout, env
            calls.append(tuple(command))
            return subprocess.CompletedProcess(command, 0, "", "")

        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata(two_public=True)),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_registry_available", side_effect=lambda *_args: next(visible)),
            mock.patch.object(
                release,
                "verify_resume_bytes",
                return_value=["sipx-sip: published bytes differ from the clean tagged archive"],
            ),
            mock.patch.object(release, "_bounded_run", side_effect=bounded),
        ):
            self.assertEqual(
                1,
                release.main(
                    (
                        "--publish",
                        "--confirm-publish",
                        tag,
                        "--command-timeout-seconds",
                        "9",
                    )
                ),
            )
        self.assertEqual([], calls)

    def test_main_registry_probe_error_dispatches_no_upload(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        calls = []

        def bounded(command: tuple[str, ...], *, cwd: pathlib.Path, timeout: float, env=None):
            del cwd, timeout, env
            calls.append(tuple(command))
            return subprocess.CompletedProcess(command, 0, "", "")

        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(
                release,
                "_registry_available",
                side_effect=release.ReleaseError("crates.io probe failed"),
            ),
            mock.patch.object(release, "_bounded_run", side_effect=bounded),
        ):
            self.assertEqual(
                1,
                release.main(("--publish", "--confirm-publish", tag)),
            )
        self.assertEqual([], calls)

    def test_main_all_visible_moved_bytes_refuse_release_success(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_registry_available", return_value=True),
            mock.patch.object(
                release,
                "verify_resume_bytes",
                return_value=["sipx-sip: published bytes differ from the clean tagged archive"],
            ) as resume,
            mock.patch.object(release, "_bounded_run") as bounded,
        ):
            self.assertEqual(1, release.main(("--publish", "--confirm-publish", tag)))
        resume.assert_called_once()
        bounded.assert_not_called()

    def test_verify_consumer_refuses_moved_bytes_before_install(self) -> None:
        version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"][
            "package"
        ]["version"]
        tag = f"v{version}"
        with (
            mock.patch.object(release, "_install_cleanup_handlers"),
            mock.patch.object(release, "_metadata", return_value=self.main_metadata()),
            mock.patch.object(release, "_checkout", return_value=(False, (tag,), (tag,))),
            mock.patch.object(release, "_registry_available", return_value=True),
            mock.patch.object(
                release,
                "verify_resume_bytes",
                return_value=["sipx-sip: published bytes differ from the clean tagged archive"],
            ) as resume,
            mock.patch.object(release, "verify_registry_consumer") as consumer,
        ):
            self.assertEqual(
                1,
                release.main(("--verify-consumer", "--registry-wait-seconds", "1")),
            )
        resume.assert_called_once()
        consumer.assert_not_called()

    def test_registry_probe_returns_absent_only_for_exact_not_found(self) -> None:
        not_found = subprocess.CompletedProcess(
            ("cargo", "info"),
            101,
            "",
            "error: could not find `sipx-sip@1.0.0-beta.1` in registry "
            "`https://github.com/rust-lang/crates.io-index`\n",
        )
        with mock.patch.object(release, "_bounded_run", return_value=not_found):
            self.assertFalse(release._registry_available("sipx-sip", "1.0.0-beta.1"))

        outage = subprocess.CompletedProcess(
            ("cargo", "info"), 101, "", "error: failed to download registry index\n"
        )
        with mock.patch.object(release, "_bounded_run", return_value=outage):
            with self.assertRaisesRegex(release.ReleaseError, "probe failed"):
                release._registry_available("sipx-sip", "1.0.0-beta.1")

    def test_dirty_wrong_tag_and_missing_confirmation_each_refuse_publication(self) -> None:
        cases = (
            ({"dirty": True, "tags": ("v1.0.0-beta.1",), "confirmation": "v1.0.0-beta.1"}, "clean"),
            ({"dirty": False, "tags": (), "confirmation": "v1.0.0-beta.1"}, "exact tag"),
            ({"dirty": False, "tags": ("v1.0.0-beta.1",), "confirmation": None}, "confirmation"),
        )
        for values, expected in cases:
            with self.subTest(expected=expected):
                problems = release.checkout_problems(
                    "publish", "1.0.0-beta.1", ci=False, **values
                )
                self.assertTrue(any(expected in problem for problem in problems), problems)

    def test_generic_ci_cannot_publish_even_with_a_clean_exact_confirmed_tag(self) -> None:
        problems = release.checkout_problems(
            "publish",
            "1.0.0-beta.1",
            dirty=False,
            tags=("v1.0.0-beta.1",),
            confirmation="v1.0.0-beta.1",
            ci=True,
        )
        self.assertTrue(any("CI" in problem for problem in problems))

    def test_exact_github_tag_push_and_tag_dispatch_are_authorized(self) -> None:
        tag = "v1.0.0-beta.1"
        sha = "a" * 40
        for event in ("push", "workflow_dispatch"):
            with self.subTest(event=event):
                self.assertEqual(
                    [],
                    release.checkout_problems(
                        "publish",
                        "1.0.0-beta.1",
                        dirty=False,
                        tags=(tag,),
                        annotated_tags=(tag,),
                        confirmation=tag,
                        ci=True,
                        head_sha=sha,
                        ci_authorization=f"{tag}@{sha}",
                        ci_environment=github_publish_environment(tag, sha, event=event),
                    ),
                )

    def test_exact_main_workflow_recovery_authorizes_one_failed_run_and_release_commit(self) -> None:
        tag = "v1.0.0-beta.1"
        release_sha = "a" * 40
        controller_sha = "b" * 40
        failed_run_id = "654321"
        self.assertEqual(
            [],
            release.checkout_problems(
                "publish",
                "1.0.0-beta.1",
                dirty=False,
                tags=(tag,),
                annotated_tags=(tag,),
                confirmation=tag,
                ci=True,
                head_sha=release_sha,
                ci_recovery_authorization=f"{tag}@{release_sha}@{failed_run_id}",
                controller_sha=controller_sha,
                ci_environment=github_recovery_environment(
                    controller_sha, failed_run_id=failed_run_id
                ),
            ),
        )

    def test_recovery_authority_cannot_start_a_first_publication(self) -> None:
        self.assertEqual(
            [],
            release.recovery_visibility_problems(None, ()),
            "ordinary publication remains allowed to start with an empty registry frontier",
        )
        problems = release.recovery_visibility_problems(
            "v1.0.0-beta.1@" + "a" * 40 + "@654321", ()
        )
        self.assertTrue(any("already registry-visible" in problem for problem in problems))
        self.assertEqual(
            [],
            release.recovery_visibility_problems(
                "v1.0.0-beta.1@" + "a" * 40 + "@654321", ("sipx-sip",)
            ),
        )

    def test_recovery_authority_is_distinct_and_every_identity_mismatch_refuses(self) -> None:
        tag = "v1.0.0-beta.1"
        release_sha = "a" * 40
        controller_sha = "b" * 40
        failed_run_id = "654321"
        base = github_recovery_environment(controller_sha, failed_run_id=failed_run_id)
        cases = (
            ("event", {"GITHUB_EVENT_NAME": "push"}, "workflow_dispatch", None),
            ("repository", {"GITHUB_REPOSITORY": "someone/sipx"}, "codewandler/sipx", None),
            ("branch", {"GITHUB_REF": f"refs/tags/{tag}"}, "refs/heads/main", None),
            (
                "workflow",
                {"GITHUB_WORKFLOW_REF": base["GITHUB_WORKFLOW_REF"].replace("resume", "release")},
                "crates-io-resume.yml",
                None,
            ),
            ("controller", {"GITHUB_SHA": "c" * 40}, "GITHUB_SHA", None),
            ("failed env", {"SIPX_FAILED_RELEASE_RUN_ID": "7"}, "failed release run", None),
            ("current run", {"GITHUB_RUN_ID": failed_run_id}, "current recovery run", None),
            ("missing token", {"CARGO_REGISTRY_TOKEN": ""}, "CARGO_REGISTRY_TOKEN", None),
            ("wrong release", {}, "recovery authorization", f"{tag}@{'c' * 40}@{failed_run_id}"),
            ("missing failed run", {}, "recovery authorization", f"{tag}@{release_sha}"),
        )
        for label, changes, expected, authorization in cases:
            environment = dict(base)
            environment.update(changes)
            with self.subTest(label=label):
                problems = release.checkout_problems(
                    "publish",
                    "1.0.0-beta.1",
                    dirty=False,
                    tags=(tag,),
                    annotated_tags=(tag,),
                    confirmation=tag,
                    ci=True,
                    head_sha=release_sha,
                    ci_recovery_authorization=(
                        authorization
                        if authorization is not None
                        else f"{tag}@{release_sha}@{failed_run_id}"
                    ),
                    controller_sha=controller_sha,
                    ci_environment=environment,
                )
                self.assertTrue(any(expected in problem for problem in problems), problems)

    def test_each_github_authority_mismatch_refuses_publication(self) -> None:
        tag = "v1.0.0-beta.1"
        sha = "a" * 40
        base = github_publish_environment(tag, sha)
        cases = (
            (
                "repository",
                {"GITHUB_REPOSITORY": "someone/sipx"},
                f"GITHUB_REPOSITORY='{release.EXPECTED_GITHUB_REPOSITORY}'",
                f"{tag}@{sha}",
            ),
            ("pull request", {"GITHUB_EVENT_NAME": "pull_request"}, "tag push", f"{tag}@{sha}"),
            ("branch ref", {"GITHUB_REF": "refs/heads/main"}, "GITHUB_REF=", f"{tag}@{sha}"),
            ("different checkout", {"GITHUB_SHA": "b" * 40}, "GITHUB_SHA", f"{tag}@{sha}"),
            (
                "workflow from main",
                {
                    "GITHUB_WORKFLOW_REF": (
                        "codewandler/sipx/.github/workflows/crates-io.yml@refs/heads/main"
                    )
                },
                "GITHUB_WORKFLOW_REF",
                f"{tag}@{sha}",
            ),
            (
                "different tagged workflow",
                {
                    "GITHUB_WORKFLOW_REF": (
                        f"codewandler/sipx/.github/workflows/ci.yml@refs/tags/{tag}"
                    )
                },
                "crates-io.yml",
                f"{tag}@{sha}",
            ),
            (
                "different workflow bytes",
                {"GITHUB_WORKFLOW_SHA": "b" * 40},
                "GITHUB_WORKFLOW_SHA",
                f"{tag}@{sha}",
            ),
            (
                "missing token",
                {"CARGO_REGISTRY_TOKEN": ""},
                "CARGO_REGISTRY_TOKEN",
                f"{tag}@{sha}",
            ),
            ("missing commit authorization", {}, "commit authorization", None),
            ("tag-only authorization", {}, "commit authorization", tag),
            ("wrong commit authorization", {}, "commit authorization", f"{tag}@{'b' * 40}"),
        )
        for label, changes, expected, authorization in cases:
            environment = dict(base)
            environment.update(changes)
            with self.subTest(label=label):
                problems = release.checkout_problems(
                    "publish",
                    "1.0.0-beta.1",
                    dirty=False,
                    tags=(tag,),
                    annotated_tags=(tag,),
                    confirmation=tag,
                    ci=True,
                    head_sha=sha,
                    ci_authorization=authorization,
                    ci_environment=environment,
                )
                self.assertTrue(any(expected in problem for problem in problems), problems)

    def test_manual_dispatch_must_resolve_the_tag_not_a_branch_head(self) -> None:
        tag = "v1.0.0-beta.1"
        sha = "a" * 40
        environment = github_publish_environment(tag, sha, event="workflow_dispatch")
        environment.update(
            {
                "GITHUB_REF": "refs/heads/main",
                "GITHUB_REF_TYPE": "branch",
                "GITHUB_REF_NAME": "main",
            }
        )
        problems = release.checkout_problems(
            "publish",
            "1.0.0-beta.1",
            dirty=False,
            tags=(tag,),
            annotated_tags=(tag,),
            confirmation=tag,
            ci=True,
            head_sha=sha,
            ci_authorization=f"{tag}@{sha}",
            ci_environment=environment,
        )
        self.assertTrue(any("refs/tags" in problem for problem in problems), problems)

    def test_ci_refusal_never_echoes_the_registry_token(self) -> None:
        tag = "v1.0.0-beta.1"
        sha = "a" * 40
        token = "do-not-print-this-fixture-token"
        environment = github_publish_environment(tag, sha)
        environment["CARGO_REGISTRY_TOKEN"] = token
        environment["GITHUB_REPOSITORY"] = "someone/sipx"
        problems = release.ci_publish_problems(
            "1.0.0-beta.1",
            head_sha=sha,
            authorization=f"{tag}@{sha}",
            environment=environment,
        )
        self.assertTrue(problems)
        self.assertNotIn(token, "\n".join(problems))

    def test_local_publish_refuses_a_ci_authorization_argument(self) -> None:
        tag = "v1.0.0-beta.1"
        sha = "a" * 40
        problems = release.checkout_problems(
            "publish",
            "1.0.0-beta.1",
            dirty=False,
            tags=(tag,),
            annotated_tags=(tag,),
            confirmation=tag,
            ci=False,
            head_sha=sha,
            ci_authorization=f"{tag}@{sha}",
        )
        self.assertTrue(any("only inside" in problem for problem in problems), problems)

    def test_ci_authorization_option_is_publish_only(self) -> None:
        with mock.patch.object(release, "_install_cleanup_handlers"):
            self.assertEqual(
                1,
                release.main(
                    ("--dry-run", "--authorize-ci-publish", "v1.0.0-beta.1@" + "a" * 40)
                ),
            )

    def test_post_publication_verification_requires_the_exact_clean_tag(self) -> None:
        problems = release.checkout_problems(
            "verify-consumer",
            "1.0.0-beta.1",
            dirty=False,
            tags=(),
            confirmation=None,
            ci=True,
        )
        self.assertTrue(any("exact tag" in problem for problem in problems))

    def test_consumer_manifest_pins_registry_crates_exactly(self) -> None:
        text = release.consumer_manifest("1.0.0-beta.1", ("sipx-sip", "sipx-call"))
        manifest = tomllib.loads(text)
        self.assertEqual(
            {"version": "=1.0.0-beta.1", "registry": "crates-io"},
            manifest["dependencies"]["sipx-sip"],
        )
        self.assertEqual(
            {"version": "=1.0.0-beta.1", "registry": "crates-io"},
            manifest["dependencies"]["sipx-call"],
        )
        self.assertNotIn("path", text)
        self.assertNotIn("git", text)

    def test_first_publish_wave_needs_no_prior_registry_checksum(self) -> None:
        self.assertEqual([], release.resume_byte_problems((), {}, {}, "a" * 40))

    def test_partial_publish_resumes_when_registry_and_tagged_bytes_match(self) -> None:
        checksum = "b" * 64
        head = "a" * 40
        evidence = {"sipx-sip": release.ArchiveEvidence(checksum, head, False)}
        self.assertEqual(
            [],
            release.resume_byte_problems(
                ("sipx-sip",), evidence, {"sipx-sip": checksum}, head
            ),
        )

    def test_moved_tag_or_changed_archive_refuses_partial_publish(self) -> None:
        old_checksum = "b" * 64
        new_checksum = "c" * 64
        old_head = "1" * 40
        moved_head = "2" * 40
        evidence = {
            "sipx-sip": release.ArchiveEvidence(new_checksum, moved_head, False),
        }
        problems = release.resume_byte_problems(
            ("sipx-sip",), evidence, {"sipx-sip": old_checksum}, old_head
        )
        self.assertTrue(any("tagged commit" in problem for problem in problems), problems)
        self.assertTrue(any("published bytes differ" in problem for problem in problems), problems)

    def test_cli_install_uses_the_exact_version_and_a_temporary_root(self) -> None:
        command = release.consumer_install_command(
            "1.0.0-beta.1", pathlib.Path("/tmp/consumer-install")
        )
        self.assertEqual(
            (
                "cargo",
                "install",
                "sipx-cli",
                "--registry",
                "crates-io",
                "--version",
                "=1.0.0-beta.1",
                "--features",
                "opus",
                "--locked",
                "--root",
                "/tmp/consumer-install",
            ),
            command,
        )

    def test_release_tag_must_be_annotated(self) -> None:
        values = {
            "dirty": False,
            "tags": ("v1.0.0-beta.1",),
            "confirmation": "v1.0.0-beta.1",
            "ci": False,
        }
        problems = release.checkout_problems("publish", "1.0.0-beta.1", **values)
        self.assertTrue(any("annotated" in problem for problem in problems))
        self.assertEqual(
            [],
            release.checkout_problems(
                "publish",
                "1.0.0-beta.1",
                annotated_tags=("v1.0.0-beta.1",),
                **values,
            ),
        )

    def test_consumer_environment_cannot_inherit_an_alternate_registry(self) -> None:
        environment = release.consumer_environment(
            pathlib.Path("/tmp/isolated-cargo"),
            {
                "PATH": "/bin",
                "CARGO_HOME": "/home/user/.cargo",
                "CARGO_REGISTRIES_CRATES_IO_INDEX": "https://invalid.example/index",
                "CARGO_SOURCE_CRATES_IO_REPLACE_WITH": "mirror",
                "CARGO_TARGET_DIR": "/shared/target",
                "CARGO_BUILD_TARGET_DIR": "/shared/build-target",
            },
        )
        self.assertEqual("/tmp/isolated-cargo", environment["CARGO_HOME"])
        self.assertEqual("sparse", environment["CARGO_REGISTRIES_CRATES_IO_PROTOCOL"])
        self.assertNotIn("CARGO_REGISTRIES_CRATES_IO_INDEX", environment)
        self.assertNotIn("CARGO_SOURCE_CRATES_IO_REPLACE_WITH", environment)
        self.assertNotIn("CARGO_TARGET_DIR", environment)
        self.assertNotIn("CARGO_BUILD_TARGET_DIR", environment)

    def test_consumer_lock_rejects_an_alternate_registry_source(self) -> None:
        lock = {
            "package": [
                {
                    "name": "sipx-sip",
                    "version": "1.0.0-beta.1",
                    "source": "registry+https://invalid.example/index",
                }
            ]
        }
        problems = release.consumer_lock_problems(lock, ("sipx-sip",), "1.0.0-beta.1")
        self.assertTrue(any("crates.io" in problem for problem in problems))

    def test_partial_readiness_line_cannot_outlive_its_bound(self) -> None:
        process = subprocess.Popen(
            (
                sys.executable,
                "-c",
                "import signal,sys; sys.stdout.write('{'); sys.stdout.flush(); signal.pause()",
            ),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        previous = signal.getsignal(signal.SIGALRM)

        def outer_bound(_number: int, _frame: object) -> None:
            raise TimeoutError("readiness reader blocked outside its own bound")

        signal.signal(signal.SIGALRM, outer_bound)
        signal.setitimer(signal.ITIMER_REAL, 1.0)
        try:
            with self.assertRaisesRegex(release.ReleaseError, "readiness"):
                release._listening_address(process, 0.1)
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0.0)
            signal.signal(signal.SIGALRM, previous)
            release._terminate_group(process)

    def test_bounded_command_reaps_its_descendant_process_group(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            pid_file = pathlib.Path(directory) / "child.pid"
            program = (
                "import pathlib,signal,subprocess,sys; "
                "child=subprocess.Popen([sys.executable,'-c','import signal; signal.pause()']); "
                "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); signal.pause()"
            )
            child_pid = None
            try:
                with self.assertRaisesRegex(release.ReleaseError, "failure bound"):
                    release._bounded_run(
                        (sys.executable, "-c", program, str(pid_file)),
                        cwd=pathlib.Path(directory),
                        timeout=0.5,
                    )
                child_pid = int(pid_file.read_text(encoding="utf-8"))
                try:
                    descriptor = os.pidfd_open(child_pid)
                except ProcessLookupError:
                    descriptor = None
                if descriptor is not None:
                    try:
                        readable, _, _ = select.select((descriptor,), (), (), 1.0)
                        self.assertTrue(readable, f"descendant {child_pid} is still running")
                    finally:
                        os.close(descriptor)
            finally:
                if child_pid is not None:
                    try:
                        os.kill(child_pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_sigterm_cleans_and_joins_owned_process_groups(self) -> None:
        program = f"""
import importlib.util, pathlib, signal, subprocess, sys
spec = importlib.util.spec_from_file_location('release_signal_test', {str(ROOT / 'scripts' / 'release.py')!r})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module._install_cleanup_handlers()
child = subprocess.Popen([sys.executable, '-c', 'import signal; signal.pause()'], start_new_session=True)
module._OWNED_GROUPS[child.pid] = child
print(child.pid, flush=True)
signal.pause()
"""
        helper = subprocess.Popen(
            (sys.executable, "-c", program),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        child_pid = None
        try:
            assert helper.stdout is not None
            ready, _, _ = select.select((helper.stdout,), (), (), 2.0)
            self.assertTrue(ready, "signal-cleanup helper did not report its child")
            child_pid = int(helper.stdout.readline())
            os.kill(helper.pid, signal.SIGTERM)
            helper.wait(timeout=7)
            helper.communicate(timeout=1)
            self.assertEqual(128 + signal.SIGTERM, helper.returncode)
            try:
                descriptor = os.pidfd_open(child_pid)
            except ProcessLookupError:
                descriptor = None
            if descriptor is not None:
                try:
                    readable, _, _ = select.select((descriptor,), (), (), 1.0)
                    self.assertTrue(readable, f"SIGTERM left descendant {child_pid} running")
                finally:
                    os.close(descriptor)
        finally:
            if helper.poll() is None:
                os.killpg(helper.pid, signal.SIGKILL)
                helper.wait(timeout=2)
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_check_and_dry_run_never_generate_a_registry_write(self) -> None:
        order = ("sipx-core", "sipx-call")
        self.assertEqual((), release.commands_for("check", order))
        commands = release.commands_for("dry-run", order, excluded=("sipx-testkit",))
        self.assertEqual(1, len(commands))
        self.assertIn("--dry-run", commands[0])
        self.assertIn("--locked", commands[0])
        self.assertIn("--workspace", commands[0])
        self.assertEqual(("--exclude", "sipx-testkit"), commands[0][-2:])

    def test_partial_availability_exposes_only_the_ready_frontier(self) -> None:
        packages = release.package_records(
            [
                package("sipx-core"),
                package("sipx-call", dependencies=(dependency("sipx-core", path="/work/crates/sipx-core"),)),
                package("sipx-cli", dependencies=(dependency("sipx-call", path="/work/crates/sipx-call"),)),
            ],
            "1.0.0-beta.1",
            pathlib.Path("/work"),
        )
        self.assertEqual(("sipx-call",), release.ready_frontier(packages, {"sipx-core"}))
        self.assertEqual(("sipx-core",), release.ready_frontier(packages, set()))
        self.assertEqual((), release.ready_frontier(packages, {"sipx-core", "sipx-call", "sipx-cli"}))

    def test_registry_visibility_polling_is_bounded_and_restartable(self) -> None:
        clock = [0.0]
        attempts = {"sipx-core": 0, "sipx-call": 0}

        def monotonic() -> float:
            return clock[0]

        def pause(seconds: float) -> None:
            clock[0] += seconds

        def probe(package: str, remaining: float) -> bool:
            self.assertGreater(remaining, 0.0)
            attempts[package] += 1
            return package == "sipx-core" and attempts[package] >= 2

        result = release.poll_registry_visibility(
            ("sipx-core", "sipx-call"),
            probe,
            timeout=5.0,
            interval=2.0,
            monotonic=monotonic,
            pause=pause,
        )
        self.assertEqual(("sipx-core",), result.available)
        self.assertEqual(("sipx-call",), result.missing)
        self.assertEqual(5.0, clock[0])

    def test_registry_visibility_can_complete_before_the_bound(self) -> None:
        clock = [0.0]
        attempts = 0

        def probe(_package: str, _remaining: float) -> bool:
            nonlocal attempts
            attempts += 1
            return attempts == 2

        result = release.poll_registry_visibility(
            ("sipx-core",),
            probe,
            timeout=10.0,
            interval=1.0,
            monotonic=lambda: clock[0],
            pause=lambda seconds: clock.__setitem__(0, clock[0] + seconds),
        )
        self.assertEqual(("sipx-core",), result.available)
        self.assertEqual((), result.missing)
        self.assertLess(clock[0], 10.0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
