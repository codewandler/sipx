#!/usr/bin/env python3
"""Tests for the public-document synchronization and content guard."""

import importlib.util
import re
import tomllib
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "sync_website", ROOT / "scripts" / "sync-website.py"
)
assert SPEC is not None and SPEC.loader is not None
SYNC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SYNC)


class GeneratedFactsTests(unittest.TestCase):
    def test_workspace_and_release_facts_come_from_canonical_files(self) -> None:
        facts = SYNC.canonical_facts()
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        workspace = manifest["workspace"]["package"]
        release = re.search(
            r"^## \[([^]]+)\] — (\d{4}-\d{2}-\d{2})$",
            (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"),
            re.MULTILINE,
        )
        self.assertIsNotNone(release)
        assert release is not None
        registry = tomllib.loads(
            (ROOT / "docs" / "rfc" / "registry.toml").read_text(encoding="utf-8")
        )
        self.assertEqual(workspace["version"], facts.workspace_version)
        self.assertEqual(workspace["rust-version"], facts.msrv)
        self.assertEqual(f"v{release.group(1)}", facts.release_tag)
        self.assertEqual(release.group(2), facts.release_date)
        self.assertEqual(len(registry["rfc"]), facts.rfc_count)

    def test_crate_map_comes_from_publishable_workspace_packages(self) -> None:
        rendered = SYNC.render_generated("crate-map", None)
        self.assertIn("| `sipx-call` | Call framework:", rendered)
        self.assertIn("| `sipx-cli` | sipx — a command line SIP softphone |", rendered)
        self.assertNotIn("`sipx-testkit`", rendered)

    def test_public_compliance_contains_every_rfc_without_internal_tracking(self) -> None:
        rendered = SYNC.render_generated("compliance", None)
        self.assertEqual(
            SYNC.canonical_facts().rfc_count,
            rendered.count("https://www.rfc-editor.org/rfc/rfc"),
        )
        self.assertNotRegex(rendered, SYNC.STORY_ID)
        self.assertNotRegex(rendered, SYNC.INTERNAL_PUBLIC_LINK)
        self.assertNotIn("a tracked change", rendered)
        self.assertNotRegex(rendered, r"Verified against [^.]+ module\.")
        self.assertIn(
            "https://github.com/codewandler/sipx/blob/main/docs/specs/srtp.md",
            rendered,
        )


class PublicGuardTests(unittest.TestCase):
    def test_guard_rejects_story_ids_and_internal_design_links(self) -> None:
        problems = SYNC.public_content_problems(
            "A-12 is internal. [design](../../docs/designs/example.md)", "sample.md"
        )
        self.assertEqual(2, len(problems))

    def test_guard_allows_rfc_and_normative_spec_links(self) -> None:
        problems = SYNC.public_content_problems(
            "RFC 3261, SHA-256, AES-128, and "
            "[our spec](https://example.invalid/docs/specs/sip.md)",
            "sample.md",
        )
        self.assertEqual([], problems)

    def test_fact_guard_rejects_stale_release_toolchain_and_rfc_copies(self) -> None:
        problems = SYNC.public_fact_problems(
            "release v0.0.0, status 0.0.0, Rust 0.0, and **999 RFCs tracked.**",
            "sample.md",
        )
        self.assertEqual(4, len(problems))


if __name__ == "__main__":
    unittest.main()
