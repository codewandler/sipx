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
import pathlib
import sys
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

    def test_exactly_one_stack_is_this_repository(self) -> None:
        _, stacks, _ = report.dataset()
        selves = [s for s in stacks if s.get("is_self")]
        self.assertEqual(1, len(selves), "exactly one stack may hold generated cells")

    def test_the_report_is_current(self) -> None:
        dimensions, stacks, observations = report.dataset()
        rendered = report.render(dimensions, stacks, observations, report.generated_values())
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
