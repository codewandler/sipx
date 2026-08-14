#!/usr/bin/env python3
"""Tests for the public-document synchronization and content guard."""

import importlib.util
import re
import tomllib
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "sync_website", ROOT / "scripts" / "sync-website.py"
)
assert SPEC is not None and SPEC.loader is not None
SYNC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SYNC)


class ChangelogCompareTests(unittest.TestCase):
    """`X-151`: release compare links follow the dated headings, not memory."""

    def test_stale_unreleased_and_missing_release_links_are_named(self) -> None:
        changelog = """\
## [Unreleased]

## [1.0.0-rc.3] — 2026-08-14

## [1.0.0-rc.1] — 2026-08-13

[Unreleased]: https://github.com/codewandler/sipx/compare/v1.0.0-rc.1...HEAD
[1.0.0-rc.1]: https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.1
"""
        problems = SYNC.changelog_compare_problems(changelog)
        self.assertEqual(2, len(problems))
        self.assertTrue(any("Unreleased" in problem for problem in problems))
        self.assertTrue(any("1.0.0-rc.3" in problem for problem in problems))

    def test_chain_follows_actual_release_order_when_a_candidate_was_skipped(self) -> None:
        changelog = """\
## [Unreleased]

## [1.0.0-rc.3] — 2026-08-14

## [1.0.0-rc.1] — 2026-08-13

[Unreleased]: https://github.com/codewandler/sipx/compare/v1.0.0-rc.3...HEAD
[1.0.0-rc.3]: https://github.com/codewandler/sipx/compare/v1.0.0-rc.1...v1.0.0-rc.3
[1.0.0-rc.1]: https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.1
"""
        self.assertEqual([], SYNC.changelog_compare_problems(changelog))

    def test_each_release_has_exactly_one_reference_definition(self) -> None:
        changelog = """\
## [Unreleased]

## [1.0.0-rc.1] — 2026-08-14

[Unreleased]: https://github.com/codewandler/sipx/compare/v1.0.0-rc.1...HEAD
[1.0.0-rc.1]: https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.1
[1.0.0-rc.1]: https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.1
"""
        problems = SYNC.changelog_compare_problems(changelog)
        self.assertEqual(1, len(problems))
        self.assertIn("2 reference links", problems[0])

    def test_repository_changelog_compare_chain_is_complete(self) -> None:
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        self.assertEqual([], SYNC.changelog_compare_problems(changelog))


class ArchitectureCoverageTests(unittest.TestCase):
    """`X-146`: the public architecture follows the shipped DSP subsystem and its specs."""

    def setUp(self) -> None:
        self.architecture = (ROOT / "website" / "docs" / "architecture.md").read_text(
            encoding="utf-8"
        )
        self.library = (
            ROOT / "website" / "docs" / "guides" / "as-a-library.md"
        ).read_text(encoding="utf-8")

    @staticmethod
    def section(page: str, heading: str) -> str:
        return page.split(heading, 1)[1].split("\n## ", 1)[0]

    def test_the_layer_diagram_names_both_dsp_owners(self) -> None:
        diagram = self.architecture.split("```mermaid", 1)[1].split("```", 1)[0]
        self.assertIn("sipx-audio::dsp", diagram)
        self.assertIn("sipx-media::dsp", diagram)

    def test_the_architecture_links_each_normative_dsp_contract_it_summarises(self) -> None:
        for spec in (
            "custom-call-dsp.md",
            "call-dsp-graph.md",
            "call-dsp-effects.md",
            "call-dsp-noise-reduction.md",
        ):
            with self.subTest(spec=spec):
                self.assertIn(f"docs/specs/{spec}", self.architecture)

    def test_both_crate_selection_tables_answer_the_two_dsp_questions(self) -> None:
        tables = (
            ("architecture", self.section(self.architecture, "## Which crate should I use?")),
            ("library", self.section(self.library, "## Which crate")),
        )
        for page, table in tables:
            with self.subTest(page=page):
                self.assertIn("`sipx-audio::dsp`", table)
                self.assertIn("`sipx-media::dsp`", table)


class GeneratedFactsTests(unittest.TestCase):
    def test_the_sdk_event_list_is_derived_from_every_normative_table_row(self) -> None:
        """`M-128`: prose cannot silently lose the next event family."""
        event_types = SYNC.app_event_types()
        rendered = SYNC.render_generated("app-event-families", None)
        self.assertGreater(len(event_types), 20)
        for event_type in event_types:
            with self.subTest(event_type=event_type):
                self.assertIn(f"`{event_type}`", rendered)
        self.assertIn("#53-event-types", rendered)

    def test_the_sdk_event_check_names_each_missing_family(self) -> None:
        rendered = SYNC.render_generated("app-event-families", None)
        missing = "call.leg.ended"
        page = (
            "<!-- BEGIN generated:app-event-families -->"
            f"{rendered.replace(f'`{missing}`', '')}"
            "<!-- END generated:app-event-families -->"
        )
        problems = SYNC.app_event_family_problems(
            page,
            "website/docs/sdk/contract.md",
        )
        self.assertEqual(1, len(problems))
        self.assertIn(missing, problems[0])

    def test_the_public_sdk_page_carries_the_complete_generated_event_list(self) -> None:
        page = (ROOT / "website" / "docs" / "sdk" / "contract.md").read_text(
            encoding="utf-8"
        )
        self.assertEqual(
            [], SYNC.app_event_family_problems(page, "website/docs/sdk/contract.md")
        )

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
        self.assertIn("| `sipx-testkit` | Deterministic SIP and RTP tests", rendered)

    def test_answer_dependencies_come_from_the_compiled_consumer(self) -> None:
        rendered = SYNC.render_generated("answer-consumer-dependencies", None)
        version = SYNC.canonical_facts().workspace_version
        self.assertIn(f'sipx-call = "={version}"', rendered)
        self.assertIn(f'sipx-sip = "={version}"', rendered)
        self.assertIn(f'sipx-transport = "={version}"', rendered)
        self.assertIn('features = ["macros", "rt-multi-thread"]', rendered)

    def test_answer_consumer_source_must_match_the_workspace_example(self) -> None:
        self.assertEqual([], SYNC.answer_consumer_source_problems("same\n", "same\n"))
        self.assertNotEqual([], SYNC.answer_consumer_source_problems("old\n", "new\n"))

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

    def test_public_comparison_matches_the_canonical_report(self) -> None:
        """The same comparison the checker just passed, or none at all."""
        rendered = SYNC.render_generated("comparison", None)
        canonical = SYNC.COMPARISON.read_text(encoding="utf-8")
        # Every stack and every dimension reaches the page, keyed off the canonical source rather
        # than off a list repeated here — a second list is a second thing to keep in step.
        for heading in re.findall(r"^## (.+)$", canonical, re.M):
            self.assertIn(heading, rendered)
        self.assertNotRegex(rendered, SYNC.STORY_ID)
        self.assertNotRegex(rendered, SYNC.INTERNAL_PUBLIC_LINK)
        self.assertNotIn("# Stack comparison", rendered)
        self.assertNotIn("Generated by scripts/comparison-report.py", rendered)

    def test_public_comparison_makes_every_internal_evidence_link_absolute(self) -> None:
        """A path relative to docs/ is dead everywhere except a checkout."""
        rendered = SYNC.render_generated("comparison", None)
        for target in re.findall(r"\]\(([^)]+)\)", rendered):
            self.assertRegex(
                target, r"^https?://", f"{target!r} is still relative on the published page"
            )
        self.assertIn("https://github.com/codewandler/sipx/blob/main/Cargo.toml", rendered)
        self.assertIn("https://github.com/codewandler/sipx/blob/main/docs/maturity.md", rendered)

    def test_public_comparison_refuses_to_render_from_a_red_checker(self) -> None:
        """The value of the page is that the check passed; publishing without it asserts a lie."""
        source = (ROOT / "scripts" / "sync-website.py").read_text(encoding="utf-8")
        body = source.split("def public_comparison")[1].split("\ndef ")[0]
        self.assertIn("returncode != 0", body)
        self.assertIn("raise ValueError", body)

    def test_the_comparison_page_is_reachable_from_the_adoption_path(self) -> None:
        """A page in the sidebar and in nobody's prose is how a reader never finds it.

        The comparison shipped registered in `sidebars.js` — so it reached the site nav, `llms.txt`
        and `llms-full.txt` — and linked from no page at all. Being orphaned is a checkable
        property, so it is checked rather than left to somebody noticing.
        """
        linking = []
        for source in SYNC.CURRENT_SURFACE_PAGES:
            if source.endswith("reference/comparison.md"):
                continue  # a page does not reach itself
            text = (ROOT / source).read_text(encoding="utf-8")
            if "reference/comparison" in text:
                linking.append(source)
        self.assertTrue(
            linking,
            "no current-surface page links to the comparison; a sidebar entry is not a path to a "
            "page",
        )

    def test_the_comparison_page_is_in_the_sidebar(self) -> None:
        """A page absent from the sidebar is silently missing from llms.txt and llms-full.txt."""
        sidebars = (ROOT / "website" / "sidebars.js").read_text(encoding="utf-8")
        self.assertIn("'reference/comparison'", sidebars)

    def test_the_comparison_page_states_where_sipx_loses(self) -> None:
        """The credibility mechanism for every other row, and the easiest thing to quietly drop."""
        page = (ROOT / "website" / "docs" / "reference" / "comparison.md").read_text(
            encoding="utf-8"
        )
        trailer = page.split("<!-- END generated:comparison -->")[1]
        self.assertIn("Where sipx loses", trailer)
        for required in ("audit", "adoption", "asymmetr", "not an interop result"):
            self.assertIn(required.lower(), trailer.lower(), f"the surround does not cover {required}")


class PublicGuardTests(unittest.TestCase):
    def test_generated_region_placement_rejects_the_broken_inline_shape(self) -> None:
        broken = (
            "<!-- BEGIN generated:workspace-version -->1.2.3"
            "<!-- END generated:workspace-version -->: suffix\n"
        )
        self.assertNotEqual(
            [], SYNC.generated_region_placement_problems(broken, "getting-started.md")
        )
        supported = (
            "Version <!-- BEGIN generated:workspace-version -->1.2.3"
            "<!-- END generated:workspace-version --> is current.\n"
        )
        self.assertEqual(
            [], SYNC.generated_region_placement_problems(supported, "getting-started.md")
        )

    def test_generated_markdown_blocks_must_be_standalone(self) -> None:
        inline = (
            "Prefix <!-- BEGIN generated:example sample.rs -->\n"
            "```rust\nfn main() {}\n```\n<!-- END generated:example -->\n"
        )
        self.assertNotEqual([], SYNC.generated_region_placement_problems(inline, "sample.md"))

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
            "current stable release", "development branch", 1
        )
        contents["website/docs/getting-started.md"] += "\nThe CLI can use UDP or TCP only.\n"
        problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("missing current stable-release status" in p for p in problems))
        self.assertTrue(any("stale current-main capability claim" in p for p in problems))

    def test_adoption_guard_rejects_the_retired_message_denial(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["website/docs/guides/does-this-fit.md"] += (
            "\nMESSAGE can be parsed but has no user-agent behavior.\n"
        )
        problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("stale current-main capability claim" in p for p in problems))

    def test_stable_v1_guard_accepts_current_prose_and_rustdoc(self) -> None:
        public_sources = set(SYNC.STABLE_V1_CURRENT_PAGES) | {
            "website/docs/whats-new.md"
        }
        public_contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in public_sources
        }
        rust_sources = set(SYNC.STABLE_V1_RUSTDOC) | set(SYNC.EXPERIMENTAL_V1_RUSTDOC)
        rust_contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in rust_sources
        }
        self.assertEqual(
            [], SYNC.stable_v1_messaging_problems(public_contents, rust_contents)
        )

    def test_stable_v1_guard_rejects_legacy_current_prose_and_rustdoc(self) -> None:
        public_contents = {
            source: "" for source in set(SYNC.STABLE_V1_CURRENT_PAGES) | {
                "website/docs/whats-new.md"
            }
        }
        public_contents["README.md"] = "Public APIs are not frozen before 1.0.\n"
        public_contents["website/docs/whats-new.md"] = (
            "## 1.0.0 — 2026-08-14\nStable.\n\n"
            "## 1.0.0-rc.23 — 2026-08-10\nThe historical prerelease remains unchanged.\n"
        )
        rust_contents = {
            source: "frozen for compatible v1 evolution"
            for source in SYNC.STABLE_V1_RUSTDOC
        }
        rust_contents.update(
            {source: "Experimental and remains unfrozen" for source in SYNC.EXPERIMENTAL_V1_RUSTDOC}
        )
        rust_contents[SYNC.STABLE_V1_RUSTDOC[0]] = "sipx is pre-1.0"
        problems = SYNC.stable_v1_messaging_problems(public_contents, rust_contents)
        self.assertTrue(any("README.md:1" in problem for problem in problems))
        self.assertTrue(any(SYNC.STABLE_V1_RUSTDOC[0] in problem for problem in problems))
        self.assertFalse(any("rc.23" in problem for problem in problems))

    def test_adoption_guard_accepts_stable_entry_points_with_the_v1_promise(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["README.md"] = contents["README.md"].replace(
            "current public prerelease", "current stable release", 1
        ).replace(
            "Public APIs are not frozen;",
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            1,
        )
        contents["website/docs/intro.md"] = contents["website/docs/intro.md"].replace(
            "current public prerelease", "current stable release", 1
        ).replace(
            "Public APIs are not frozen before 1.0:",
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            1,
        )
        stable = SYNC.canonical_facts()._replace(workspace_version="1.0.0")
        with mock.patch.object(SYNC, "canonical_facts", return_value=stable):
            self.assertEqual([], SYNC.public_adoption_problems(contents))

    def test_adoption_guard_rejects_prerelease_policy_on_a_stable_version(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["README.md"] = contents["README.md"].replace(
            "current stable release", "current public prerelease", 1
        ).replace(
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            "Public APIs are not frozen;",
            1,
        )
        contents["website/docs/intro.md"] = contents["website/docs/intro.md"].replace(
            "current stable release", "current public prerelease", 1
        ).replace(
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            "Public APIs are not frozen before 1.0:",
            1,
        )
        stable = SYNC.canonical_facts()._replace(workspace_version="1.0.0")
        with mock.patch.object(SYNC, "canonical_facts", return_value=stable):
            problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("missing current stable-release status" in p for p in problems))
        self.assertTrue(any("still claims a public prerelease" in p for p in problems))
        self.assertTrue(any("still claims Supported APIs are not frozen" in p for p in problems))

    def test_stable_policy_keeps_shared_adoption_and_capability_guards(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["README.md"] = contents["README.md"].replace(
            "current public prerelease", "current stable release", 1
        ).replace(
            "Public APIs are not frozen;",
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            1,
        )
        contents["website/docs/intro.md"] = contents["website/docs/intro.md"].replace(
            "current public prerelease", "current stable release", 1
        ).replace(
            "Public APIs are not frozen before 1.0:",
            "Supported Rust APIs remain source-compatible throughout the v1 line.",
            1,
        )
        contents["website/docs/whats-new.md"] = contents[
            "website/docs/whats-new.md"
        ].replace("published as exact crates.io packages", "available from source", 1)
        contents["website/docs/getting-started.md"] += "\nThe CLI can use UDP or TCP only.\n"
        stable = SYNC.canonical_facts()._replace(workspace_version="1.0.0")
        with mock.patch.object(SYNC, "canonical_facts", return_value=stable):
            problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("missing registry honesty" in p for p in problems))
        self.assertTrue(any("stale current-main capability claim" in p for p in problems))

    def test_prerelease_policy_rejects_a_concurrent_stable_status(self) -> None:
        sources = set(SYNC.ADOPTION_REQUIREMENTS) | set(SYNC.CURRENT_SURFACE_PAGES)
        contents = {
            source: (ROOT / source).read_text(encoding="utf-8") for source in sources
        }
        contents["README.md"] += "\nThis is the current stable release.\n"
        prerelease = SYNC.canonical_facts()._replace(workspace_version="1.0.0-rc.24")
        with mock.patch.object(SYNC, "canonical_facts", return_value=prerelease):
            problems = SYNC.public_adoption_problems(contents)
        self.assertTrue(any("still claims a stable release" in p for p in problems))


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
