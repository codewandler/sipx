#!/usr/bin/env python3
"""Adversarial fixtures for ``check-packaged-opus.py``."""

from __future__ import annotations

import importlib.util
import pathlib
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "check_packaged_opus", ROOT / "scripts/check-packaged-opus.py"
)
assert SPEC is not None and SPEC.loader is not None
proof = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(proof)


def manifest(name: str, opus: list[str], *, default: list[str] | None = None) -> dict[str, object]:
    result: dict[str, object] = {
        "package": {"name": name, "license": "MIT OR Apache-2.0"},
        "features": {"default": default or [], "opus": opus},
        "dependencies": {},
    }
    if name == "sipx-audio":
        result["dependencies"] = {"opus": {"optional": True}}
    return result


class ArchiveBoundary(unittest.TestCase):
    def test_member_must_stay_below_exact_package_prefix(self) -> None:
        for member in ("elsewhere/Cargo.toml", "/pkg/Cargo.toml", "pkg/../secret"):
            with self.subTest(member=member), self.assertRaises(proof.ProofError):
                proof.archive_destination("pkg", member)

    def test_regular_member_resolves_relative_to_package(self) -> None:
        self.assertEqual(
            pathlib.PurePosixPath("src/main.rs"),
            proof.archive_destination("pkg", "pkg/src/main.rs"),
        )


class FeatureBoundary(unittest.TestCase):
    def setUp(self) -> None:
        self.manifests = {
            "sipx-cli": manifest("sipx-cli", ["sipx-call/opus", "sipx-media/opus"]),
            "sipx-call": manifest("sipx-call", ["sipx-media/opus"]),
            "sipx-media": manifest("sipx-media", ["sipx-audio/opus"]),
            "sipx-audio": manifest("sipx-audio", ["dep:opus"]),
        }

    def test_complete_normalized_chain_passes(self) -> None:
        self.assertEqual([], proof.feature_chain_problems(self.manifests))

    def test_cli_that_drops_forwarding_fails(self) -> None:
        self.manifests["sipx-cli"] = manifest("sipx-cli", ["sipx-call/opus"])
        problems = proof.feature_chain_problems(self.manifests)
        self.assertTrue(any("sipx-media/opus" in problem for problem in problems))

    def test_native_binding_must_stay_optional(self) -> None:
        self.manifests["sipx-audio"]["dependencies"] = {"opus": {"optional": False}}
        problems = proof.feature_chain_problems(self.manifests)
        self.assertTrue(any("not optional" in problem for problem in problems))

    def test_default_feature_cannot_turn_opus_on(self) -> None:
        self.manifests["sipx-audio"] = manifest(
            "sipx-audio", ["dep:opus"], default=["opus"]
        )
        problems = proof.feature_chain_problems(self.manifests)
        self.assertTrue(any("must remain opt-in" in problem for problem in problems))

    def test_resolved_graph_proves_both_sides(self) -> None:
        self.assertEqual(
            [],
            proof.graph_problems("sipx-cli v1\nsipx-call v1\n", "opus v0.3\naudiopus_sys v0.2\n"),
        )
        self.assertTrue(proof.graph_problems("opus v0.3\n", "opus v0.3\n"))

    def test_root_help_must_come_from_the_running_binary(self) -> None:
        self.assertEqual([], proof.help_problems("USAGE:\n    sipx [OPTIONS] [COMMAND]\n"))
        self.assertTrue(proof.help_problems("finished compiling sipx-cli"))


class PublicPolicy(unittest.TestCase):
    def test_each_policy_fact_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            guide = root / "website/docs/guides/as-a-library.md"
            guide.parent.mkdir(parents=True)
            guide.write_text("off by default\n", encoding="utf-8")
            problems = proof.policy_documentation_problems(root)
            self.assertEqual(3, len(problems))


if __name__ == "__main__":
    unittest.main()
