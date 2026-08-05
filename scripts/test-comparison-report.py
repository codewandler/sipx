#!/usr/bin/env python3
"""Tests for comparison-report.py — the checker that keeps the comparison a measurement.

Every rule the checker enforces gets four tests: the real artifact satisfies it, a reversed
fixture produces the *specific* problem, a legitimate record is not flagged, and the claim
reaches the rendered document. A guard that has only the first kind cannot tell whether it is
guarding.

Fixture stacks are named `zz-fixture-*` on purpose. This file is **not** inside
`COMPARISON_SCOPE` (see `scripts/check-provenance.sh`), so a real comparison subject written
into a fixture here would be caught by the provenance check — the same reason
`test-provenance.py` invents its own term.
"""

import datetime
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_report_module():
    """Import comparison-report.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "comparison_report", ROOT / "scripts" / "comparison-report.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


report = load_report_module()

#: The reserved fixture identity. Assertions filter on it so a fixture's problem can never be
#: confused with a problem the real dataset has.
FIXTURE_STACK = "zz-fixture-stack"
FIXTURE_DIMENSION = "zz-fixture-dimension"
FIXTURE_REVISION = "0123456789abcdef0123456789abcdef01234567"
TODAY = datetime.date(2026, 8, 4)


def a_dimension(**overrides):
    """A minimal well-formed dimension, so a test can vary exactly one thing about it."""
    dimension = {
        "id": FIXTURE_DIMENSION,
        "title": "A tracked dimension",
        "question": "What does this dimension ask?",
        "why": "Because a dimension that cannot say why it is here is a column, not a question.",
    }
    dimension.update(overrides)
    return dimension


def a_stack(**overrides):
    """A minimal well-formed stack."""
    stack = {
        "id": FIXTURE_STACK,
        "name": "A tracked stack",
        "language": "Rust",
        "repository": "https://example.invalid/zz-fixture-stack",
        "license": "MIT",
    }
    stack.update(overrides)
    return stack


def an_observation(**overrides):
    """A minimal well-formed observation at the `documented` tier."""
    observation = {
        "stack": FIXTURE_STACK,
        "dimension": FIXTURE_DIMENSION,
        "confidence": "documented",
        "summary": "The subject's own documentation states the thing.",
        "evidence": [{"url": "https://example.invalid/doc", "note": "the subject's manual"}],
        "version_evaluated": "1.2.3",
        "evaluated_at": "2026-08-01",
    }
    observation.update(overrides)
    return observation


def problems_for(observation, *, stacks=None, dimensions=None, today=TODAY):
    """Run the whole checker over one observation and return only this fixture's problems."""
    found = report.check(
        dimensions if dimensions is not None else [a_dimension()],
        stacks if stacks is not None else [a_stack()],
        [observation],
        report.GENERATED_VALUES_FOR_TESTS,
        today,
    )
    return [p for p in found if FIXTURE_STACK in p or FIXTURE_DIMENSION in p]


def a_capability(**overrides):
    capability = {
        "id": "zz-capability",
        "category": "core",
        "title": "A public capability",
        "ownership": "sipx",
        "status": "implemented",
        "confidence": "measured",
        "implementation": ["crates/sipx-sip/src/message.rs"],
        "evidence": [
            {
                "url": f"https://example.invalid/source/{FIXTURE_REVISION}",
                "note": "the exported API",
            }
        ],
    }
    capability.update(overrides)
    return capability


def a_capability_ledger(capabilities=None, **overrides):
    capabilities = capabilities or [a_capability()]
    ledger = {
        "subject": FIXTURE_STACK,
        "version_evaluated": "1.2.3",
        "evaluated_at": "2026-08-01",
        "source_revision": FIXTURE_REVISION,
        "expected_capabilities": len(capabilities),
        "capabilities": capabilities,
        "_file": f"{FIXTURE_STACK}.json",
    }
    ledger.update(overrides)
    return ledger


def capability_problems_for(capability, **ledger_overrides):
    ledger = a_capability_ledger([capability], **ledger_overrides)
    return checked_capability_problems([ledger])


def expectations_for(ledgers):
    return {
        ledger["subject"]: (
            ledger["source_revision"],
            {capability["id"] for capability in ledger["capabilities"]},
        )
        for ledger in ledgers
    }


def checked_capability_problems(ledgers, stacks=None):
    return report.capability_problems(
        ledgers,
        stacks or [a_stack()],
        TODAY,
        expectations=expectations_for(ledgers),
    )


def a_complete_capability_ledger():
    capabilities = []
    for category in sorted(report.REQUIRED_CAPABILITY_CATEGORIES):
        capabilities.append(a_capability(id=f"zz-{category}", category=category))
    return a_capability_ledger(capabilities)


class TheCapabilityLedger(unittest.TestCase):
    """A complete leaf inventory has one evidence-backed owner and disposition per row."""

    def test_an_open_sipx_row_without_a_story_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(status="open"))
        self.assertTrue(any("open sipx row" in problem for problem in problems), problems)

    def test_a_complete_fresh_ledger_is_accepted(self) -> None:
        self.assertEqual(
            [],
            checked_capability_problems([a_complete_capability_ledger()]),
        )

    def test_an_unknown_owner_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(ownership="somebody"))
        self.assertTrue(any("unknown ownership" in problem for problem in problems), problems)

    def test_an_unknown_subject_is_rejected(self) -> None:
        ledger = a_complete_capability_ledger()
        problems = checked_capability_problems(
            [ledger], [a_stack(id="another-stack")]
        )
        self.assertTrue(any("does not declare" in problem for problem in problems), problems)

    def test_a_mismatched_filename_is_rejected(self) -> None:
        ledger = a_complete_capability_ledger()
        ledger["_file"] = "wrong.json"
        problems = checked_capability_problems([ledger])
        self.assertTrue(any("filename must match" in problem for problem in problems), problems)

    def test_a_missing_revision_is_rejected(self) -> None:
        ledger = a_complete_capability_ledger()
        ledger["source_revision"] = ""
        problems = checked_capability_problems([ledger])
        self.assertTrue(any("no immutable version" in problem for problem in problems), problems)

    def test_an_invalid_owner_status_pair_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(status="tracked"))
        self.assertTrue(any("not valid for ownership" in problem for problem in problems), problems)

    def test_a_duplicate_leaf_is_rejected(self) -> None:
        capability = a_capability()
        ledger = a_capability_ledger([capability, dict(capability)])
        problems = checked_capability_problems([ledger])
        self.assertTrue(any("declares capability" in problem for problem in problems), problems)

    def test_a_duplicate_subject_ledger_is_rejected(self) -> None:
        ledger = a_complete_capability_ledger()
        problems = checked_capability_problems([ledger, dict(ledger)])
        self.assertTrue(any("has 2 ledgers" in problem for problem in problems), problems)

    def test_the_expected_count_ratchets_leaf_removal(self) -> None:
        ledger = a_complete_capability_ledger()
        ledger["capabilities"].pop()
        problems = checked_capability_problems([ledger])
        self.assertTrue(any("expected capabilities" in problem for problem in problems), problems)

    def test_the_separate_exact_id_inventory_survives_a_coedited_count(self) -> None:
        ledger = a_complete_capability_ledger()
        expectations = expectations_for([ledger])
        removed = ledger["capabilities"].pop()["id"]
        ledger["expected_capabilities"] -= 1
        problems = report.capability_problems(
            [ledger], [a_stack()], TODAY, expectations=expectations
        )
        self.assertTrue(
            any(removed in problem and "omits expected" in problem for problem in problems),
            problems,
        )

    def test_an_unevidenced_leaf_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(evidence=[]))
        self.assertTrue(any("cites no evidence" in problem for problem in problems), problems)

    def test_an_implemented_leaf_without_rust_source_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(implementation=[]))
        self.assertTrue(any("Rust source evidence" in problem for problem in problems), problems)

    def test_implementation_evidence_must_be_workspace_rust_source(self) -> None:
        problems = capability_problems_for(a_capability(implementation=["README.md"]))
        self.assertTrue(any("workspace crate" in problem for problem in problems), problems)

    def test_non_sipx_rows_cannot_carry_implementation_evidence(self) -> None:
        problems = capability_problems_for(
            a_capability(ownership="not-shipped", status="absent")
        )
        self.assertTrue(
            any("without implemented sipx ownership" in problem for problem in problems),
            problems,
        )

    def test_an_open_row_with_a_done_story_is_rejected(self) -> None:
        problems = capability_problems_for(
            a_capability(
                status="open",
                implementation=None,
                story="docs/stories/M-42-advertise-a-chosen-address-and-latch-rtp-without-ice.md",
            )
        )
        self.assertTrue(any("is done" in problem for problem in problems), problems)

    def test_an_open_row_with_a_non_story_file_is_rejected(self) -> None:
        problems = capability_problems_for(
            a_capability(status="open", implementation=None, story="README.md")
        )
        self.assertTrue(any("has no status" in problem for problem in problems), problems)

    def test_a_cluster_story_must_be_in_the_pinned_index(self) -> None:
        problems = capability_problems_for(
            a_capability(
                ownership="sipx-clstr",
                status="tracked",
                implementation=None,
                story="https://example.invalid/story.md",
            )
        )
        self.assertTrue(any("pinned external index" in problem for problem in problems), problems)

    def test_every_required_category_must_remain(self) -> None:
        ledger = a_complete_capability_ledger()
        ledger["capabilities"] = [
            row for row in ledger["capabilities"] if row["category"] != "transports"
        ]
        ledger["expected_capabilities"] = len(ledger["capabilities"])
        problems = checked_capability_problems([ledger])
        self.assertTrue(any("omits required categories" in problem for problem in problems), problems)

    def test_a_stale_ledger_is_rejected(self) -> None:
        stale = TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS + 1)
        problems = capability_problems_for(
            a_capability(), evaluated_at=stale.isoformat()
        )
        self.assertTrue(any("stale" in problem for problem in problems), problems)

    def test_an_exclusion_without_a_rationale_is_rejected(self) -> None:
        problems = capability_problems_for(
            a_capability(ownership="not-applicable", status="excluded")
        )
        self.assertTrue(any("without a rationale" in problem for problem in problems), problems)

    def test_an_unknown_confidence_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(confidence="certain"))
        self.assertTrue(any("unknown confidence" in problem for problem in problems), problems)

    def test_assessed_confidence_requires_a_rationale(self) -> None:
        problems = capability_problems_for(a_capability(confidence="assessed"))
        self.assertTrue(any("assessed without a rationale" in problem for problem in problems), problems)

    def test_measured_confidence_requires_the_exact_source_revision(self) -> None:
        problems = capability_problems_for(
            a_capability(
                evidence=[
                    {
                        "url": "https://example.invalid/source/v1.2.3",
                        "note": "a mutable tag",
                    }
                ]
            )
        )
        self.assertTrue(any("without pinning" in problem for problem in problems), problems)

    def test_documented_confidence_may_cite_versioned_prose(self) -> None:
        capability = a_capability(
            confidence="documented",
            evidence=[
                {
                    "url": "https://example.invalid/docs/v1.2.3",
                    "note": "the subject documentation",
                }
            ],
        )
        self.assertFalse(
            any("without pinning" in problem for problem in capability_problems_for(capability))
        )

    def test_schema_rejects_non_string_evidence_paths(self) -> None:
        capability = a_capability(evidence=[{"path": 123, "note": "not a path"}])
        problems = report.capability_schema_problems(a_capability_ledger([capability]))
        self.assertTrue(any("evidence path value" in problem for problem in problems), problems)

    def test_schema_rejects_empty_optional_implementation_lists(self) -> None:
        capability = a_capability(status="open", story="README.md", implementation=[])
        problems = report.capability_schema_problems(a_capability_ledger([capability]))
        self.assertTrue(any("implementation list" in problem for problem in problems), problems)

    def test_scalar_schema_constraints_are_checked(self) -> None:
        capability = a_capability(
            id="",
            title="",
            evidence=[{"url": "not a uri", "note": ""}],
        )
        ledger = a_capability_ledger([capability], source_revision="x")
        problems = report.capability_schema_problems(ledger)
        problems.extend(report.capability_evidence_problems(ledger, capability))
        for phrase in (
            "source revision",
            "stable capability key",
            "empty title",
            "empty note",
            "invalid evidence URL",
        ):
            self.assertTrue(
                any(phrase in problem for problem in problems), (phrase, problems)
            )

    def test_a_leaf_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack()],
            [an_observation()],
            report.GENERATED_VALUES_FOR_TESTS,
            [a_capability_ledger()],
        )
        self.assertIn("A public capability", rendered)
        self.assertIn("Endpoint capability ledger", rendered)


class TheExternalStoryIndex(unittest.TestCase):
    """Cluster ownership cites an exact commit, path and Git blob identity offline."""

    def valid_index(self):
        return {
            "repository": "https://example.invalid/cluster",
            "source_revision": "a" * 40,
            "stories": [
                {
                    "path": "docs/stories/ZZ-1-a-story.md",
                    "blob_sha": "b" * 40,
                }
            ],
        }

    def problems_for(self, value):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            (directory / "cluster.json").write_text(json.dumps(value), encoding="utf-8")
            return report.external_story_index_problems(directory)

    def test_a_complete_pinned_index_is_accepted_and_derives_the_url(self) -> None:
        value = self.valid_index()
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            (directory / "cluster.json").write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual([], report.external_story_index_problems(directory))
            self.assertEqual(
                {
                    "https://example.invalid/cluster/blob/"
                    f"{'a' * 40}/docs/stories/ZZ-1-a-story.md"
                },
                report.external_story_urls(directory),
            )

    def test_each_external_index_refusal_has_a_mutated_fixture(self) -> None:
        mutations = []
        value = self.valid_index()
        value["extra"] = True
        mutations.append((value, "unknown key"))
        value = self.valid_index()
        value["repository"] = "git@example.invalid:cluster"
        mutations.append((value, "repository URL"))
        value = self.valid_index()
        value["source_revision"] = "main"
        mutations.append((value, "source revision"))
        value = self.valid_index()
        value["stories"] = []
        mutations.append((value, "no story paths"))
        value = self.valid_index()
        value["stories"] = ["docs/stories/ZZ-1-a-story.md"]
        mutations.append((value, "not an object"))
        value = self.valid_index()
        value["stories"][0]["extra"] = True
        mutations.append((value, "require exactly"))
        value = self.valid_index()
        value["stories"][0]["path"] = "README.md"
        mutations.append((value, "invalid story path"))
        value = self.valid_index()
        value["stories"][0]["blob_sha"] = "not-a-blob"
        mutations.append((value, "blob identity"))
        value = self.valid_index()
        value["stories"].append(dict(value["stories"][0]))
        mutations.append((value, "repeats story path"))

        for value, phrase in mutations:
            with self.subTest(phrase=phrase):
                problems = self.problems_for(value)
                self.assertTrue(any(phrase in problem for problem in problems), problems)


class TheExactCapabilityInventory(unittest.TestCase):
    """Expected IDs are reviewed separately from the disposition ledger they ratchet."""

    def valid_inventory(self):
        return {
            "subject": FIXTURE_STACK,
            "source_revision": FIXTURE_REVISION,
            "expected_ids": ["zz-capability"],
        }

    def load(self, value, filename=f"{FIXTURE_STACK}.json"):
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            (directory / filename).write_text(json.dumps(value), encoding="utf-8")
            return report.capability_expectations(directory)

    def test_a_complete_exact_id_inventory_is_accepted(self) -> None:
        expectations, problems = self.load(self.valid_inventory())
        self.assertEqual([], problems)
        self.assertEqual(
            (FIXTURE_REVISION, {"zz-capability"}), expectations[FIXTURE_STACK]
        )

    def test_each_exact_inventory_refusal_has_a_mutated_fixture(self) -> None:
        mutations = []
        value = self.valid_inventory()
        value["extra"] = True
        mutations.append((value, f"{FIXTURE_STACK}.json", "requires exactly"))
        mutations.append((self.valid_inventory(), "wrong.json", "subject or filename"))
        value = self.valid_inventory()
        value["source_revision"] = "main"
        mutations.append((value, f"{FIXTURE_STACK}.json", "source revision"))
        value = self.valid_inventory()
        value["expected_ids"] = []
        mutations.append((value, f"{FIXTURE_STACK}.json", "no expected"))
        value = self.valid_inventory()
        value["expected_ids"] = ["NOT A KEY"]
        mutations.append((value, f"{FIXTURE_STACK}.json", "invalid capability"))
        value = self.valid_inventory()
        value["expected_ids"] *= 2
        mutations.append((value, f"{FIXTURE_STACK}.json", "repeats a capability"))

        for value, filename, phrase in mutations:
            with self.subTest(phrase=phrase):
                _, problems = self.load(value, filename)
                self.assertTrue(any(phrase in problem for problem in problems), problems)


class TheClosedKeySet(unittest.TestCase):
    """A record may carry the keys its schema names, and no others."""

    def test_a_well_formed_observation_is_accepted(self) -> None:
        self.assertEqual([], problems_for(an_observation()))

    def test_an_unknown_key_is_rejected(self) -> None:
        problems = problems_for(an_observation(verdict="better"))
        self.assertTrue(
            any("verdict" in p and "unknown key" in p for p in problems),
            f"an unknown key was accepted in silence; problems={problems}",
        )

    def test_score_is_rejected_with_its_own_hint(self) -> None:
        """The one somebody adds on purpose, so the message argues rather than just refuses."""
        problems = problems_for(an_observation(score=7))
        self.assertTrue(
            any("score" in p and "confidence" in p for p in problems),
            f"a weighted score was refused without saying why; problems={problems}",
        )

    def test_a_missing_required_key_is_rejected(self) -> None:
        observation = an_observation()
        del observation["summary"]
        problems = problems_for(observation)
        self.assertTrue(
            any("summary" in p and "missing" in p for p in problems),
            f"a missing summary was accepted; problems={problems}",
        )

    def test_a_dimension_may_not_carry_an_unknown_key(self) -> None:
        problems = report.schema_problems("dimension", a_dimension(weight=3))
        self.assertTrue(
            any("weight" in p and "unknown key" in p for p in problems),
            f"a weighted dimension was accepted; problems={problems}",
        )


class TheConfidenceLadder(unittest.TestCase):
    """Each tier carries an obligation, and the checker holds the row to it."""

    def test_generated_on_a_stack_that_is_not_this_repository_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(
                confidence="generated",
                generated_from=["rfc-count"],
                summary="It tracks {rfc-count} documents.",
            )
        )
        self.assertTrue(
            any("generated" in p and "is_self" in p for p in problems),
            f"an external stack claimed a generated cell; problems={problems}",
        )

    def test_generated_is_accepted_on_this_repository(self) -> None:
        problems = problems_for(
            an_observation(
                confidence="generated",
                generated_from=["rfc-count"],
                summary="It tracks {rfc-count} documents.",
                # A generated cell is computed from the current tree, so it has to name that
                # tree's version — see `TheSelfVersion`.
                version_evaluated=report.workspace_version(),
            ),
            stacks=[a_stack(is_self=True)],
        )
        self.assertEqual([], problems)

    def test_measured_without_a_reproduce_command_is_rejected(self) -> None:
        problems = problems_for(an_observation(confidence="measured"))
        self.assertTrue(
            any("measured" in p and "reproduce" in p for p in problems),
            f"a measurement nobody can re-run was accepted; problems={problems}",
        )

    def test_measured_with_a_reproduce_command_is_accepted(self) -> None:
        problems = problems_for(
            an_observation(confidence="measured", reproduce="grep -c thing src/")
        )
        self.assertEqual([], problems)

    def test_assessed_without_a_rationale_is_rejected(self) -> None:
        problems = problems_for(an_observation(confidence="assessed"))
        self.assertTrue(
            any("assessed" in p and "rationale" in p for p in problems),
            f"a judgment with no reasoning was accepted; problems={problems}",
        )

    def test_assessed_with_a_rationale_is_accepted(self) -> None:
        problems = problems_for(
            an_observation(confidence="assessed", rationale="Read from the release notes only.")
        )
        self.assertEqual([], problems)

    def test_an_unknown_tier_is_rejected(self) -> None:
        problems = problems_for(an_observation(confidence="probably"))
        self.assertTrue(
            any("probably" in p for p in problems),
            f"an invented confidence tier was accepted; problems={problems}",
        )

    def test_every_tier_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack()],
            [an_observation(confidence="assessed", rationale="Indirect reading.")],
            report.GENERATED_VALUES_FOR_TESTS,
        )
        self.assertIn("assessed", rendered)


class EvidenceMustBeAbleToFail(unittest.TestCase):
    """Prose is not evidence here, as everywhere else in this repository."""

    def test_an_observation_with_no_evidence_is_rejected(self) -> None:
        problems = problems_for(an_observation(evidence=[]))
        self.assertTrue(
            any("evidence" in p for p in problems),
            f"an unevidenced claim was accepted; problems={problems}",
        )

    def test_evidence_naming_neither_a_url_nor_a_path_is_rejected(self) -> None:
        problems = problems_for(an_observation(evidence=[{"note": "trust me"}]))
        self.assertTrue(
            any("url" in p and "path" in p for p in problems),
            f"an evidence entry pointing nowhere was accepted; problems={problems}",
        )

    def test_evidence_naming_both_a_url_and_a_path_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(
                evidence=[{"url": "https://example.invalid/x", "path": "README.md", "note": "n"}]
            )
        )
        self.assertTrue(
            any("url" in p and "path" in p for p in problems),
            f"an ambiguous evidence entry was accepted; problems={problems}",
        )

    def test_a_repository_path_that_does_not_exist_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(evidence=[{"path": "crates/nothing-here.rs", "note": "n"}])
        )
        self.assertTrue(
            any("nothing-here" in p and "exist" in p for p in problems),
            f"a citation of a missing file was accepted; problems={problems}",
        )

    def test_a_repository_path_that_exists_is_accepted(self) -> None:
        problems = problems_for(
            an_observation(evidence=[{"path": "README.md", "note": "the front page"}])
        )
        self.assertEqual([], problems)

    def test_evidence_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack()],
            [an_observation()],
            report.GENERATED_VALUES_FOR_TESTS,
        )
        self.assertIn("https://example.invalid/doc", rendered)


class TheStalenessGate(unittest.TestCase):
    """A comparison ages the moment it ships, and refusing to report is the honest answer."""

    def test_a_missing_evaluation_date_is_rejected(self) -> None:
        observation = an_observation()
        del observation["evaluated_at"]
        problems = problems_for(observation)
        self.assertTrue(
            any("evaluated_at" in p for p in problems),
            f"an undated observation was accepted; problems={problems}",
        )

    def test_a_missing_evaluated_version_is_rejected(self) -> None:
        observation = an_observation()
        del observation["version_evaluated"]
        problems = problems_for(observation)
        self.assertTrue(
            any("version_evaluated" in p for p in problems),
            f"an unpinned observation was accepted; problems={problems}",
        )

    def test_an_unparseable_date_is_rejected(self) -> None:
        problems = problems_for(an_observation(evaluated_at="last summer"))
        self.assertTrue(
            any("last summer" in p or "YYYY-MM-DD" in p for p in problems),
            f"an unparseable date was accepted; problems={problems}",
        )

    def test_an_observation_past_the_age_limit_is_rejected(self) -> None:
        stale = TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS + 1)
        problems = problems_for(an_observation(evaluated_at=stale.isoformat()))
        self.assertTrue(
            any("stale" in p for p in problems),
            f"a stale observation was published; problems={problems}",
        )

    def test_the_staleness_message_names_the_refresh_command(self) -> None:
        """A red gate on a date must be actionable, or it becomes the thing people silence."""
        stale = TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS + 1)
        problems = problems_for(an_observation(evaluated_at=stale.isoformat()))
        self.assertTrue(
            any(report.REFRESH_COMMAND in p for p in problems),
            f"the staleness failure did not say how to fix it; problems={problems}",
        )

    def test_a_fresh_observation_is_not_flagged(self) -> None:
        fresh = TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS - 1)
        self.assertEqual([], problems_for(an_observation(evaluated_at=fresh.isoformat())))

    def test_the_evaluated_version_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack()],
            [an_observation(version_evaluated="9.8.7")],
            report.GENERATED_VALUES_FOR_TESTS,
        )
        self.assertIn("9.8.7", rendered)


class TheStalenessWarning(unittest.TestCase):
    """A wall with no notice is the failure people learn to silence, so it gets a notice."""

    def warnings_for(self, observation, today=TODAY):
        found = report.expiring_soon([observation], today)
        return [w for w in found if FIXTURE_STACK in w]

    def test_an_observation_inside_the_band_warns(self) -> None:
        soon = TODAY - datetime.timedelta(
            days=report.MAX_OBSERVATION_AGE_DAYS - report.STALE_WARNING_DAYS + 1
        )
        warnings = self.warnings_for(an_observation(evaluated_at=soon.isoformat()))
        self.assertTrue(
            warnings, "an observation about to expire gave no notice at all"
        )
        self.assertTrue(
            any(report.REFRESH_COMMAND in w for w in warnings),
            f"the notice did not say how to act on it; warnings={warnings}",
        )

    def test_an_observation_inside_the_band_does_not_fail_the_build(self) -> None:
        """A warning that fails the build is a wall that arrives 30 days early."""
        soon = TODAY - datetime.timedelta(
            days=report.MAX_OBSERVATION_AGE_DAYS - report.STALE_WARNING_DAYS + 1
        )
        self.assertEqual([], problems_for(an_observation(evaluated_at=soon.isoformat())))

    def test_an_observation_outside_the_band_is_silent(self) -> None:
        fresh = TODAY - datetime.timedelta(
            days=report.MAX_OBSERVATION_AGE_DAYS - report.STALE_WARNING_DAYS - 1
        )
        self.assertEqual([], self.warnings_for(an_observation(evaluated_at=fresh.isoformat())))

    def test_the_band_did_not_replace_the_wall(self) -> None:
        """Past the limit is still a failure, and still names the refresh command."""
        stale = TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS + 1)
        problems = problems_for(an_observation(evaluated_at=stale.isoformat()))
        self.assertTrue(any("stale" in p for p in problems), f"problems={problems}")
        self.assertTrue(any(report.REFRESH_COMMAND in p for p in problems), f"problems={problems}")

    def test_a_marker_is_never_warned_about(self) -> None:
        """A dimension nobody evaluated has no evidence to go stale."""
        marker = {"stack": FIXTURE_STACK, "dimension": FIXTURE_DIMENSION, "not_evaluated": "no"}
        self.assertEqual([], self.warnings_for(marker))

    def test_the_countdown_reports_the_soonest_expiry(self) -> None:
        older = TODAY - datetime.timedelta(days=100)
        newer = TODAY - datetime.timedelta(days=10)
        days = report.days_until_expiry(
            [
                an_observation(evaluated_at=older.isoformat()),
                an_observation(evaluated_at=newer.isoformat()),
            ],
            TODAY,
        )
        self.assertEqual(report.MAX_OBSERVATION_AGE_DAYS - 100, days)

    def test_the_countdown_reaches_the_success_line(self) -> None:
        """Present on every green run, not only near the limit."""
        source = (ROOT / "scripts" / "comparison-report.py").read_text(encoding="utf-8")
        main_body = source.split("def main(")[1]
        self.assertIn("days_until_expiry", main_body)
        self.assertIn("next expires in", main_body)


class AbsenceIsNeverAmbiguous(unittest.TestCase):
    """A blank cell must say whether nobody looked or nothing was found."""

    def test_a_dimension_with_no_observation_is_rejected(self) -> None:
        found = report.check(
            [a_dimension()], [a_stack()], [], report.GENERATED_VALUES_FOR_TESTS, TODAY
        )
        problems = [p for p in found if FIXTURE_STACK in p]
        self.assertTrue(
            any("not_evaluated" in p for p in problems),
            f"a silently empty cell was accepted; problems={problems}",
        )

    def test_an_explicit_not_evaluated_marker_is_accepted(self) -> None:
        marker = {"stack": FIXTURE_STACK, "dimension": FIXTURE_DIMENSION, "not_evaluated": "no"}
        self.assertEqual([], problems_for(marker))

    def test_a_not_evaluated_marker_may_not_also_make_a_claim(self) -> None:
        marker = {
            "stack": FIXTURE_STACK,
            "dimension": FIXTURE_DIMENSION,
            "not_evaluated": "no source access",
            "summary": "but also it is great",
        }
        problems = problems_for(marker)
        self.assertTrue(
            any("not_evaluated" in p and "summary" in p for p in problems),
            f"a marker smuggled in a claim; problems={problems}",
        )

    def test_an_empty_not_evaluated_reason_is_rejected(self) -> None:
        marker = {"stack": FIXTURE_STACK, "dimension": FIXTURE_DIMENSION, "not_evaluated": ""}
        problems = problems_for(marker)
        self.assertTrue(
            any("not_evaluated" in p for p in problems),
            f"an unexplained omission was accepted; problems={problems}",
        )

    def test_a_duplicate_pair_is_rejected(self) -> None:
        found = report.check(
            [a_dimension()],
            [a_stack()],
            [an_observation(), an_observation(summary="a second, different answer")],
            report.GENERATED_VALUES_FOR_TESTS,
            TODAY,
        )
        problems = [p for p in found if FIXTURE_STACK in p]
        self.assertTrue(
            any("one answer per dimension" in p for p in problems),
            f"one stack answered one question twice; problems={problems}",
        )

    def test_an_observation_against_an_unknown_dimension_is_rejected(self) -> None:
        problems = problems_for(an_observation(dimension="zz-fixture-nowhere"))
        self.assertTrue(
            any("zz-fixture-nowhere" in p for p in problems),
            f"an orphan observation was accepted; problems={problems}",
        )

    def test_the_not_evaluated_reason_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack()],
            [{"stack": FIXTURE_STACK, "dimension": FIXTURE_DIMENSION, "not_evaluated": "no tag"}],
            report.GENERATED_VALUES_FOR_TESTS,
        )
        self.assertIn("no tag", rendered)


class GeneratedCellsAreNeverTyped(unittest.TestCase):
    """This repository's own column is computed at render time, so it cannot be hand-edited."""

    def test_a_generated_cell_must_name_the_rules_it_interpolates(self) -> None:
        problems = problems_for(
            an_observation(
                confidence="generated",
                generated_from=["rfc-count"],
                summary="It tracks 72 documents.",
            ),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(
            any("rfc-count" in p and "placeholder" in p for p in problems),
            f"a generated cell typed its own number; problems={problems}",
        )

    def test_a_placeholder_with_no_declared_rule_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(
                confidence="generated",
                generated_from=["rfc-count"],
                summary="It tracks {rfc-count} documents over {gate-steps} steps.",
            ),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(
            any("gate-steps" in p for p in problems),
            f"an undeclared placeholder was accepted; problems={problems}",
        )

    def test_an_unknown_rule_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(
                confidence="generated",
                generated_from=["vibes"],
                summary="It is {vibes}.",
            ),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(
            any("vibes" in p for p in problems),
            f"an invented generation rule was accepted; problems={problems}",
        )

    def test_a_non_generated_cell_may_not_interpolate(self) -> None:
        problems = problems_for(an_observation(summary="It tracks {rfc-count} documents."))
        self.assertTrue(
            any("rfc-count" in p for p in problems),
            f"an external cell borrowed this repository's generated value; problems={problems}",
        )

    def test_generated_from_without_the_generated_tier_is_rejected(self) -> None:
        problems = problems_for(
            an_observation(generated_from=["rfc-count"]),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(
            any("generated_from" in p for p in problems),
            f"a documented cell claimed a generation rule; problems={problems}",
        )

    def test_the_recomputed_value_reaches_the_rendered_document(self) -> None:
        rendered = report.render(
            [a_dimension()],
            [a_stack(is_self=True)],
            [
                an_observation(
                    confidence="generated",
                    generated_from=["rfc-count"],
                    summary="It tracks {rfc-count} documents.",
                )
            ],
            {"rfc-count": "1234"},
        )
        self.assertIn("1234", rendered)
        self.assertNotIn("{rfc-count}", rendered)


class TheSelfVersion(unittest.TestCase):
    """A generated cell is computed from the current tree, so it must say which tree that is."""

    def a_generated_observation(self, **overrides):
        return an_observation(
            confidence="generated",
            generated_from=["rfc-count"],
            summary="It tracks {rfc-count} documents.",
            **overrides,
        )

    def test_a_generated_cell_at_a_stale_version_is_rejected(self) -> None:
        problems = problems_for(
            self.a_generated_observation(version_evaluated="0.0.0-not-the-workspace"),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(
            any("workspace" in p and "0.0.0-not-the-workspace" in p for p in problems),
            f"a generated cell claimed a version it was not computed from; problems={problems}",
        )

    def test_the_message_names_the_remedy(self) -> None:
        problems = problems_for(
            self.a_generated_observation(version_evaluated="0.0.0-not-the-workspace"),
            stacks=[a_stack(is_self=True)],
        )
        self.assertTrue(any("regenerate" in p for p in problems), f"problems={problems}")

    def test_a_generated_cell_at_the_workspace_version_is_accepted(self) -> None:
        problems = problems_for(
            self.a_generated_observation(version_evaluated=report.workspace_version()),
            stacks=[a_stack(is_self=True)],
        )
        self.assertEqual([], problems)

    def test_an_external_row_may_name_any_version(self) -> None:
        """The rule is about our own computed cells, not about anyone else's pinned tag."""
        self.assertEqual([], problems_for(an_observation(version_evaluated="2.17")))

    def test_the_workspace_version_is_read_from_the_manifest(self) -> None:
        import tomllib

        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        self.assertEqual(
            manifest["workspace"]["package"]["version"], report.workspace_version()
        )


class TheGenerationRules(unittest.TestCase):
    """Each rule reads a live source, so a cell cannot outlive the fact behind it."""

    def test_every_rule_produces_a_value_from_the_real_repository(self) -> None:
        values = report.generated_values()
        self.assertEqual(sorted(report.GENERATED_RULES), sorted(values))
        for rule, value in values.items():
            self.assertTrue(value.strip(), f"rule {rule} produced nothing")

    def test_the_rfc_count_matches_the_registry(self) -> None:
        import tomllib

        entries = tomllib.loads((ROOT / "docs" / "rfc" / "registry.toml").read_text())["rfc"]
        self.assertEqual(str(len(entries)), report.generated_values()["rfc-count"])

    def test_the_transport_list_matches_the_enum(self) -> None:
        source = (ROOT / "crates" / "sipx-transport" / "src" / "target.rs").read_text()
        self.assertIn("enum TransportKind", source)
        for token in report.generated_values()["transports"].split(", "):
            self.assertIn(f'"{token}"', source, f"{token} is not a spelling the enum emits")


class TheRealDataset(unittest.TestCase):
    """The guard is only worth having if the dataset it guards already satisfies it."""

    def test_the_dataset_has_no_outstanding_problems(self) -> None:
        dimensions, stacks, observations = report.dataset()
        self.assertEqual(
            [],
            report.check(
                dimensions,
                stacks,
                observations,
                report.generated_values(),
                datetime.date.today(),
            ),
        )

    def test_the_capability_ledgers_have_no_outstanding_problems(self) -> None:
        _, stacks, _ = report.dataset()
        expectations, expectation_problems = report.capability_expectations()
        self.assertEqual([], expectation_problems)
        self.assertEqual(
            [],
            report.capability_problems(
                report.capability_ledgers(),
                stacks,
                datetime.date.today(),
                report.external_story_urls(),
                expectations,
            ),
        )

    def test_exactly_one_stack_is_this_repository(self) -> None:
        _, stacks, _ = report.dataset()
        selves = [s for s in stacks if s.get("is_self")]
        self.assertEqual(1, len(selves), "exactly one stack may hold generated cells")

    def test_the_report_is_current(self) -> None:
        dimensions, stacks, observations = report.dataset()
        rendered = report.render(
            dimensions,
            stacks,
            observations,
            report.generated_values(),
            report.capability_ledgers(),
        )
        self.assertEqual(
            report.REPORT.read_text(encoding="utf-8"),
            rendered,
            "docs/comparison.md is out of date; run ./scripts/comparison-report.py",
        )

    def test_every_schema_file_is_valid_json(self) -> None:
        import json

        for path in sorted((ROOT / "docs" / "comparison" / "schema").glob("*.schema.json")):
            loaded = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(
                "https://json-schema.org/draft/2020-12/schema", loaded.get("$schema"), path.name
            )


class TheScriptItself(unittest.TestCase):
    """Structural rules that a data-driven checker has to keep to stay data-driven."""

    def test_no_stack_identity_is_written_into_the_script(self) -> None:
        """The script is outside COMPARISON_SCOPE, so a subject name in it fails provenance."""
        source = (ROOT / "scripts" / "comparison-report.py").read_text(encoding="utf-8")
        _, stacks, _ = report.dataset()
        for stack in stacks:
            if stack.get("is_self"):
                continue
            self.assertNotIn(
                stack["id"],
                source,
                "the checker must read subjects from stacks.json, never name one",
            )

    def test_there_is_no_suppression_list(self) -> None:
        source = (ROOT / "scripts" / "comparison-report.py").read_text(encoding="utf-8")
        for word in ("EXCEPTIONS", "ALLOWLIST", "IGNORED", "SUPPRESS"):
            self.assertNotIn(
                word, source, "the only escape for an unevidenced claim is demotion or removal"
            )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
