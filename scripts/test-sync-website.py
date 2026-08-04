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

    def test_badge_numbers_come_from_the_sources_that_govern_them(self) -> None:
        """A badge is the most-read line in a README and the least likely to be re-checked."""
        facts = SYNC.canonical_facts()
        registry = tomllib.loads(
            (ROOT / "docs" / "rfc" / "registry.toml").read_text(encoding="utf-8")
        )
        implemented = sum(1 for row in registry["rfc"] if row.get("status") == "implemented")
        self.assertEqual(implemented, facts.rfc_implemented)
        # `partial` is never folded in as a fraction, for `docs/maturity.md`'s reason: one number
        # would call a fully implemented row and a partial one the same thing.
        self.assertLess(facts.rfc_implemented, facts.rfc_count)

        rendered = SYNC.render_generated("badges", None)
        self.assertIn(f"message={facts.workspace_version}&", rendered)
        self.assertIn(f"{facts.rfc_implemented}%20implemented%20of%20{facts.rfc_count}", rendered)
        # The path form escapes a hyphen by doubling it, which would publish a version string
        # nobody can install. The query form is what keeps the badge literal.
        self.assertNotIn("--alpha", rendered)
        self.assertNotIn("/badge/", rendered)

    def test_the_codec_badge_reports_what_the_audio_gate_asserts(self) -> None:
        """Not a second opinion about the codecs — the same one, or no badge at all."""
        codecs = SYNC.claimed_codecs()
        self.assertIn("G.711", codecs)
        rendered = SYNC.render_generated("badges", None)
        for codec in codecs:
            self.assertIn(codec, rendered)

    def test_every_badge_link_resolves_to_a_real_readme_anchor_or_file(self) -> None:
        """`X-41` made a dead anchor fail the build; a badge is not exempt from that."""
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        anchors = {
            "#" + re.sub(r"[^a-z0-9 -]", "", line.lstrip("# ").lower()).replace(" ", "-")
            for line in readme.splitlines()
            if line.startswith("#")
        }
        for href in re.findall(r'<a href="([^"]+)">', SYNC.render_generated("badges", None)):
            if href.startswith("http"):
                continue
            if href.startswith("#"):
                self.assertIn(href, anchors, f"badge links to a missing anchor {href}")
            else:
                self.assertTrue((ROOT / href).is_file(), f"badge links to a missing file {href}")

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

    def test_fact_guard_requires_exact_current_registry_versions(self) -> None:
        version = SYNC.canonical_facts().workspace_version
        problems = SYNC.public_fact_problems(
            f"cargo install --version {version} sipx-cli\n"
            f'sipx-call = "{version}"',
            "README.md",
        )
        self.assertEqual(2, len(problems))
        self.assertTrue(all("exact" in problem for problem in problems))

    def test_fact_guard_rejects_stale_registry_versions(self) -> None:
        problems = SYNC.public_fact_problems(
            "cargo install --version =1.0.0-alpha.5 sipx-cli\n"
            'sipx-call = "=1.0.0-alpha.5"',
            "website/docs/getting-started.md",
        )
        self.assertEqual(2, len(problems))
        self.assertTrue(all("differs from workspace version" in problem for problem in problems))

    def test_fact_guard_preserves_historical_release_versions(self) -> None:
        historical = (
            "## 1.0.0-alpha.5 — 2026-08-03\n\n"
            "Install v1.0.0-alpha.5 with:\n\n"
            "cargo install --version =1.0.0-alpha.5 sipx-cli\n"
        )
        self.assertEqual(
            [], SYNC.public_fact_problems(historical, "website/docs/whats-new.md")
        )
        self.assertNotEqual([], SYNC.public_fact_problems(historical, "README.md"))

    def test_adoption_guard_accepts_the_current_public_entry_points(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        self.assertEqual([], SYNC.public_adoption_problems(contents))

    def test_adoption_guard_rejects_a_missing_policy_and_stale_capability(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["README.md"] = contents["README.md"].replace(
            "current public-beta release", "development branch", 1
        )
        contents["website/docs/getting-started.md"] += "\nThe CLI can use UDP or TCP only.\n"
        problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("missing published public-beta status" in p for p in problems))
        self.assertTrue(any("stale current-main capability claim" in p for p in problems))


class RustDocExampleGuardTests(unittest.TestCase):
    def test_rust_doc_examples_refuse_panic_access_indexing_and_detached_tasks(self) -> None:
        sample = """\
//! ```no_run
//! let address = "127.0.0.1".parse().expect("address");
//! let first = packets[0];
//! tokio::spawn(async move { serve().await });
//! ```
"""
        problems = SYNC.rust_doc_example_problems(sample, "sample.rs")
        self.assertEqual(3, len(problems))
        self.assertTrue(any("panic-prone" in problem for problem in problems))
        self.assertTrue(any("raw indexing" in problem for problem in problems))
        self.assertTrue(any("detached task" in problem for problem in problems))

    def test_non_rust_history_and_owned_tasks_are_allowed(self) -> None:
        sample = """\
//! ```text
//! value.unwrap()
//! ```
//! ```
//! let mut tasks = tokio::task::JoinSet::new();
//! tasks.spawn(async move { serve().await });
//! let value = values.get(0);
//! ```
"""
        self.assertEqual([], SYNC.rust_doc_example_problems(sample, "sample.rs"))


if __name__ == "__main__":
    unittest.main()
