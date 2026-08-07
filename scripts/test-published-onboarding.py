#!/usr/bin/env python3
"""Tests for the published onboarding consumer and rendered-page assertions."""

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "published_onboarding", ROOT / "scripts" / "check-published-onboarding.py"
)
assert SPEC is not None and SPEC.loader is not None
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ConsumerTests(unittest.TestCase):
    def inputs(self) -> tuple[str, str, str, str, str]:
        version, edition = CHECK.workspace_facts()
        fixture = CHECK.FIXTURE
        return (
            (fixture / "Cargo.toml").read_text(encoding="utf-8"),
            (fixture / "src" / "main.rs").read_text(encoding="utf-8"),
            CHECK.EXAMPLE.read_text(encoding="utf-8"),
            version,
            edition,
        )

    def test_archived_consumer_is_complete_and_registry_shaped(self) -> None:
        manifest, source, example, version, edition = self.inputs()
        self.assertEqual(
            [],
            CHECK.source_problems(
                manifest, source, example, version=version, edition=edition
            ),
        )

    def test_missing_direct_dependency_reproduces_the_review_failure(self) -> None:
        manifest, source, example, version, edition = self.inputs()
        missing = manifest.replace(f'sipx-sip = "={version}"\n', "")
        problems = CHECK.source_problems(
            missing, source, example, version=version, edition=edition
        )
        self.assertTrue(any("sipx-sip" in problem for problem in problems), problems)

    def test_path_dependency_and_source_drift_are_refused(self) -> None:
        manifest, source, example, version, edition = self.inputs()
        leaked = manifest.replace(
            f'sipx-call = "={version}"',
            f'sipx-call = {{ version = "={version}", path = "../../crates/sipx-call" }}',
        )
        problems = CHECK.source_problems(
            leaked, source + "\n", example, version=version, edition=edition
        )
        self.assertTrue(any("path" in problem for problem in problems), problems)
        self.assertTrue(any("differs" in problem for problem in problems), problems)


class BuiltPageTests(unittest.TestCase):
    def test_complete_sentence_is_one_visible_paragraph(self) -> None:
        version = "1.2.3-rc.4"
        good = (
            "<html><body><p>Confirm which version was installed. "
            f"This documentation build covers {version}:</p></body></html>"
        )
        self.assertEqual([], CHECK.built_page_problems(good, version))

    def test_split_and_truncated_sentence_reproduces_the_site_failure(self) -> None:
        version = "1.2.3-rc.4"
        broken = (
            "<p>Confirm which version was installed. This documentation build covers</p>"
            "<p>.2.3-rc.4:</p>"
        )
        self.assertNotEqual([], CHECK.built_page_problems(broken, version))


if __name__ == "__main__":
    unittest.main()
