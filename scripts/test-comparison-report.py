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
import os
import pathlib
import signal
import sys
import tempfile
import threading
import time
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


def load_comparative_module():
    """Import the neutral load contract kept beside the comparison checker."""
    spec = importlib.util.spec_from_file_location(
        "comparative_load", ROOT / "scripts" / "comparative-load.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


load_contract = load_comparative_module()

#: The reserved fixture identity. Assertions filter on it so a fixture's problem can never be
#: confused with a problem the real dataset has.
FIXTURE_STACK = "zz-fixture-stack"
FIXTURE_DIMENSION = "zz-fixture-dimension"
TODAY = datetime.date(2026, 8, 4)


def process_group_exists(pgid):
    """Observe whether a POSIX process group still owns at least one process."""
    try:
        os.killpg(pgid, 0)
    except ProcessLookupError:
        return False
    return True


def force_group_cleanup(pgid, timeout_seconds=2.0):
    """Keep adversarial supervision fixtures from leaving work behind when an assertion fails."""
    try:
        os.killpg(pgid, signal.SIGKILL)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + timeout_seconds
    pause = threading.Event()
    while process_group_exists(pgid) and time.monotonic() < deadline:
        pause.wait(min(0.01, max(0.0, deadline - time.monotonic())))


def a_load_manifest():
    """One complete immutable v1 execution manifest."""
    return {
        "schema": load_contract.MANIFEST_SCHEMA,
        "run_id": "0123456789abcdef0123456789abcdef",
        "seed": 7,
        "direction": {"index": 0, "driver": "endpoint-a", "responder": "endpoint-b"},
        "builds": [
            {
                "endpoint_id": "endpoint-a",
                "role": "driver",
                "revision": "revision-a",
                "artifact_sha256": "a" * 64,
                "argv": ["/opt/endpoint-a", "drive"],
                "cwd": "/opt",
                "env_keys": ["PATH"],
            },
            {
                "endpoint_id": "endpoint-b",
                "role": "responder",
                "revision": "revision-b",
                "artifact_sha256": "b" * 64,
                "argv": ["/opt/endpoint-b", "respond"],
                "cwd": "/opt",
                "env_keys": ["PATH"],
            },
        ],
        "machine": {
            "os": "fixture-os",
            "architecture": "fixture-arch",
            "logical_cpus": 8,
            "memory_bytes": 8 * 1024 * 1024 * 1024,
            "clock": "monotonic",
        },
        "ceiling": 1024,
        "provisional_policy": "trying_100",
        "limits": {
            "active": 2048,
            "events": load_contract.MAX_EVENTS,
            "event_bytes": load_contract.MAX_EVENT_BYTES,
            "stdout_bytes": load_contract.MAX_LOG_BYTES,
            "stderr_bytes": load_contract.MAX_LOG_BYTES,
        },
        "phases": {
            "readiness_ms": load_contract.READINESS_MS,
            "correctness_rate": 1,
            "correctness_dialogs": 20,
            "headroom_multiplier": 2,
            "warmup_ms": load_contract.WARMUP_MS,
            "measurement_ms": load_contract.MEASUREMENT_MS,
            "drain_ms": load_contract.MAX_DRAIN_MS,
        },
        "ladder": {
            "divisors": list(load_contract.LADDER_DIVISORS),
            "repetitions": load_contract.REPETITIONS,
            "stop_after_failed_rates": load_contract.STOP_AFTER_FAILED_RATES,
        },
    }


def a_load_result(manifest=None):
    """A passed result with complete post-cleanup and resource evidence."""
    manifest = manifest or a_load_manifest()
    build = manifest["builds"][0]
    offered = 1920
    return {
        "schema": load_contract.RESULT_SCHEMA,
        "status": "passed",
        "run": {
            "run_id": manifest["run_id"],
            "seed": manifest["seed"],
            "direction": manifest["direction"],
            "rate_index": 0,
            "rate_per_second": 32,
            "repetition": 0,
            "started_utc": "2026-08-05T12:00:00Z",
            "elapsed_ms": 70_100,
            "warmup_ms": load_contract.WARMUP_MS,
            "measurement_ms": load_contract.MEASUREMENT_MS,
            "drain_ms": 100,
        },
        "build": {
            "endpoint_id": build["endpoint_id"],
            "role": build["role"],
            "revision": build["revision"],
            "artifact_sha256": build["artifact_sha256"],
            "argv_sha256": load_contract.argv_hash(build["argv"]),
        },
        "machine": manifest["machine"],
        "profile": {
            "transport": "udp",
            "t1_ms": 500,
            "t2_ms": 4000,
            "t4_ms": 5000,
            "provisional_policy": manifest["provisional_policy"],
            "maximum_active": manifest["limits"]["active"],
            "events": manifest["limits"]["events"],
            "event_bytes": manifest["limits"]["event_bytes"],
            "stdout_bytes": manifest["limits"]["stdout_bytes"],
            "stderr_bytes": manifest["limits"]["stderr_bytes"],
            "contract_sha256": load_contract.contract_hash(),
        },
        "counts": {
            "offered": offered,
            "established": offered,
            "completed": offered,
            "active_high_water": 64,
            "request_retransmissions": 0,
            "response_retransmissions": 0,
        },
        "responses": {"provisional": {"100": offered}, "final": {"200": offered * 2}},
        "errors": {name: 0 for name in load_contract.TERMINAL_ERRORS + load_contract.RUN_ERRORS},
        "latency_ms": {
            "setup": {"count": offered, "p50": 2, "p95": 4, "p99": 6, "max": 8},
            "teardown": {"count": offered, "p50": 1, "p95": 2, "p99": 3, "max": 5},
        },
        "resources": {
            "sample_interval_ms": 100,
            "unsupported_resources": [],
            "cpu_user_ms": 10_000,
            "cpu_system_ms": 2_000,
            "peak_rss_bytes": 64 * 1024 * 1024,
            "descriptor_high_water": 32,
            "task_thread_high_water": 16,
            "endpoint_active_high_water": 64,
        },
        "post_drain": {
            "active_dialogs": 0,
            "transactions": 0,
            "timers": 0,
            "endpoint_tasks": 0,
            "retained_events": 0,
        },
        "cleanup": {
            "admission_stopped": True,
            "zero_state_observed": True,
            "process_group_exited": True,
            "leader_status": 0,
            "descendant_pipe_eof": True,
            "escalation": "none",
            "elapsed_ms": 100,
        },
    }


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


class TheComparativeLoadContract(unittest.TestCase):
    """X-98's fixed profile, evidence schema and process-group cleanup are executable rules."""

    def assert_manifest_refused(self, manifest) -> None:
        with self.assertRaises(load_contract.ContractError):
            load_contract.validate_manifest(manifest)

    def assert_result_refused(self, result, manifest=None) -> None:
        with self.assertRaises(load_contract.ContractError):
            load_contract.validate_result(result, manifest or a_load_manifest())

    @staticmethod
    def changed(value):
        return json.loads(json.dumps(value))

    def test_the_exact_profile_and_complete_post_cleanup_result_are_accepted(self) -> None:
        manifest = a_load_manifest()
        self.assertIs(load_contract.validate_manifest(manifest), manifest)
        result = a_load_result(manifest)
        self.assertIs(load_contract.validate_result(result, manifest), result)

    def test_every_dialog_identifier_including_ack_and_to_is_deterministic(self) -> None:
        self.assertEqual(
            {
                "call_id": "cl-0123456789abcdef0123456789abcdef-3@driver.invalid",
                "from_tag": "f-dbcde7aba829a6d2",
                "to_tag": "t-f8d0e81e93174798",
                "invite_branch": "z9hG4bK-i-2a0029d75e3b140c398b",
                "ack_branch": "z9hG4bK-a-9cf65817dc9741e3da13",
                "bye_branch": "z9hG4bK-b-d211c0e00a0ac3affb69",
            },
            load_contract.dialog_identifiers(
                7, "0123456789abcdef0123456789abcdef", 3
            ),
        )

    def test_the_spec_carries_exact_ack_bye_and_bye_response_templates(self) -> None:
        text = (ROOT / "docs" / "specs" / "comparative-load.md").read_text(encoding="utf-8")
        self.assertIn("ACK sip:load@<responder-uri> SIP/2.0\\r\\n", text)
        self.assertIn("Via: SIP/2.0/UDP <driver-via>;rport;branch=<ack-branch>\\r\\n", text)
        self.assertIn("BYE sip:load@<responder-uri> SIP/2.0\\r\\n", text)
        self.assertIn("CSeq: 2 BYE\\r\\n", text)
        self.assertIn("To tag: t-<first-16-hex", text)

    def test_zero_missing_or_widened_phase_bounds_are_rejected(self) -> None:
        original = a_load_manifest()
        for name, value in (("drain_ms", 0), ("measurement_ms", 0), ("warmup_ms", 10_001)):
            changed = self.changed(original)
            changed["phases"][name] = value
            self.assert_manifest_refused(changed)
        changed = self.changed(original)
        del changed["phases"]["readiness_ms"]
        self.assert_manifest_refused(changed)

    def test_the_manifest_fixes_one_closed_provisional_response_policy(self) -> None:
        manifest = a_load_manifest()
        for invalid in (None, True, "sometimes", "180_ringing"):
            changed = self.changed(manifest)
            changed["provisional_policy"] = invalid
            self.assert_manifest_refused(changed)

        changed = self.changed(manifest)
        changed["provisional_policy"] = "none"
        load_contract.validate_manifest(changed)

        missing = self.changed(manifest)
        del missing["provisional_policy"]
        self.assert_manifest_refused(missing)

    def test_incomplete_identity_machine_and_hash_metadata_are_rejected(self) -> None:
        manifest = a_load_manifest()
        changed = self.changed(manifest)
        del changed["machine"]["architecture"]
        self.assert_manifest_refused(changed)
        result = a_load_result(manifest)
        changed_result = self.changed(result)
        changed_result["build"]["artifact_sha256"] = "0" * 64
        self.assert_result_refused(changed_result, manifest)
        changed_result = self.changed(result)
        changed_result["build"]["argv_sha256"] = "0" * 64
        self.assert_result_refused(changed_result, manifest)

    def test_invalid_utc_phase_totals_and_response_totals_are_rejected(self) -> None:
        manifest = a_load_manifest()
        result = a_load_result(manifest)

        changed = self.changed(result)
        changed["run"]["started_utc"] = "not-a-time"
        self.assert_result_refused(changed, manifest)
        changed["run"]["started_utc"] = "2026-08-05T12Z"
        self.assert_result_refused(changed, manifest)

        changed = self.changed(result)
        changed["run"]["elapsed_ms"] = (
            changed["run"]["warmup_ms"]
            + changed["run"]["measurement_ms"]
            + changed["run"]["drain_ms"]
            - 1
        )
        self.assert_result_refused(changed, manifest)

        changed = self.changed(result)
        changed["responses"] = {"provisional": {}, "final": {}}
        self.assert_result_refused(changed, manifest)

        changed = self.changed(result)
        changed["responses"]["final"]["200"] += 1
        self.assert_result_refused(changed, manifest)

        changed = self.changed(result)
        changed["responses"]["final"]["486"] = 1
        self.assert_result_refused(changed, manifest)

        changed = self.changed(result)
        changed["responses"]["final"]["201"] = 1
        changed["responses"]["final"]["200"] -= 1
        self.assert_result_refused(changed, manifest)

    def test_exact_rejection_and_provisional_response_accounting_is_accepted(self) -> None:
        manifest = a_load_manifest()
        result = a_load_result(manifest)
        result["status"] = "failed"
        result["counts"]["completed"] -= 2
        result["errors"]["rejected"] = 1
        result["errors"]["admission_refused"] = 1
        result["latency_ms"]["teardown"]["count"] -= 2
        result["responses"]["final"]["200"] -= 2
        result["responses"]["final"].update({"486": 1, "503": 1})
        load_contract.validate_result(result, manifest)

        no_trying = self.changed(manifest)
        no_trying["provisional_policy"] = "none"
        no_trying_result = a_load_result(no_trying)
        no_trying_result["responses"]["provisional"] = {}
        load_contract.validate_result(no_trying_result, no_trying)

        contradictory = a_load_result(manifest)
        contradictory["responses"]["provisional"]["100"] -= 1
        self.assert_result_refused(contradictory, manifest)

    def test_missing_cleanup_or_live_post_drain_state_cannot_pass(self) -> None:
        result = a_load_result()
        changed = self.changed(result)
        del changed["cleanup"]
        self.assert_result_refused(changed)
        changed = self.changed(result)
        changed["post_drain"]["endpoint_tasks"] = 1
        self.assert_result_refused(changed)
        changed = self.changed(result)
        changed["cleanup"]["descendant_pipe_eof"] = False
        self.assert_result_refused(changed)

    def test_passed_status_requires_clean_unforced_process_exit(self) -> None:
        result = a_load_result()

        changed = self.changed(result)
        changed["cleanup"]["leader_status"] = 1
        self.assert_result_refused(changed)

        changed = self.changed(result)
        changed["cleanup"]["leader_status"] = -signal.SIGKILL
        self.assert_result_refused(changed)

        changed = self.changed(result)
        changed["cleanup"]["escalation"] = "kill"
        self.assert_result_refused(changed)

    def test_process_crash_count_and_leader_status_must_agree(self) -> None:
        result = a_load_result()
        result["status"] = "failed"

        crashed_without_accounting = self.changed(result)
        crashed_without_accounting["cleanup"]["leader_status"] = 2
        self.assert_result_refused(crashed_without_accounting)

        accounting_without_crash = self.changed(result)
        accounting_without_crash["errors"]["process_crash"] = 1
        self.assert_result_refused(accounting_without_crash)

        result["cleanup"]["leader_status"] = 2
        result["errors"]["process_crash"] = 1
        load_contract.validate_result(result, a_load_manifest())

    def test_unsupported_resources_are_absent_not_zero(self) -> None:
        result = a_load_result()
        changed = self.changed(result)
        changed["resources"]["unsupported_resources"] = ["cpu_user_ms"]
        changed["resources"]["cpu_user_ms"] = 0
        self.assert_result_refused(changed)
        del changed["resources"]["cpu_user_ms"]
        load_contract.validate_result(changed, a_load_manifest())

    def test_two_consecutive_failed_rates_omit_only_the_higher_rates(self) -> None:
        self.assertEqual((), load_contract.omitted_after([True, False]))
        self.assertEqual((3, 4, 5), load_contract.omitted_after([True, False, False]))
        self.assertEqual((), load_contract.omitted_after([False, True, False]))
        with self.assertRaises(load_contract.ContractError):
            load_contract.omitted_after([True] * 7)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_cleanup_terminates_a_blocking_descendant_and_observes_pipe_eof(self) -> None:
        helper = """
import json, os, signal, subprocess, sys
subprocess.Popen([sys.executable, '-c', 'import signal; signal.pause()'])
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        old_sigint = load_contract.signal.getsignal(load_contract.signal.SIGINT)
        with load_contract.ProcessSupervisor(cleanup_wait_seconds=0.25) as owner:
            supervised = owner.start(
                [sys.executable, "-c", helper],
                "responder",
                stdout_limit=4096,
                stderr_limit=4096,
            )
            ready = supervised.wait_ready(timeout_ms=2_000)
            self.assertEqual(supervised.process.pid, ready["pid"])
            self.assertNotEqual(
                old_sigint, load_contract.signal.getsignal(load_contract.signal.SIGINT)
            )
        self.assertTrue(supervised.stdout.eof.is_set())
        self.assertTrue(supervised.stderr.eof.is_set())
        self.assertIsNotNone(supervised.process.returncode)
        self.assertEqual(old_sigint, load_contract.signal.getsignal(load_contract.signal.SIGINT))

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_cleanup_observes_group_exit_when_descendant_closed_its_pipes(self) -> None:
        child = "import signal; signal.signal(signal.SIGTERM, signal.SIG_IGN); signal.pause()"
        with tempfile.TemporaryDirectory() as directory:
            pid_file = pathlib.Path(directory) / "child.pid"
            helper = f"""
import json, os, pathlib, subprocess, sys
child = subprocess.Popen(
    [sys.executable, '-c', {child!r}],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
pathlib.Path({str(pid_file)!r}).write_text(str(child.pid), encoding='ascii')
print(json.dumps({{
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {{'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096}},
}}), flush=True)
"""
            supervised = load_contract.SupervisedProcess(
                [sys.executable, "-c", helper],
                "responder",
                stdout_limit=4096,
                stderr_limit=4096,
            )
            pgid = supervised.pgid
            try:
                supervised.wait_ready(timeout_ms=2_000)
                self.assertTrue(pid_file.read_text(encoding="ascii"))
                self.assertEqual("kill", supervised.close(timeout_seconds=0.25))
                self.assertFalse(process_group_exists(pgid))
            finally:
                force_group_cleanup(pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_a_failed_graceful_callback_still_forces_complete_cleanup(self) -> None:
        helper = """
import json, os, signal
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        def fail_to_stop_orderly():
            raise RuntimeError("orderly stop failed")

        supervised = load_contract.SupervisedProcess(
            [sys.executable, "-c", helper],
            "responder",
            graceful=fail_to_stop_orderly,
            stdout_limit=4096,
            stderr_limit=4096,
        )
        pgid = supervised.pgid
        worker_pid = supervised.orderly_stop_worker_pid
        try:
            supervised.wait_ready(timeout_ms=2_000)
            with self.assertRaisesRegex(load_contract.ContractError, "orderly stop failed"):
                supervised.close(timeout_seconds=0.25)
            self.assertIsNotNone(supervised.process.returncode)
            self.assertFalse(process_group_exists(pgid))
            self.assertIsNotNone(worker_pid)
            self.assertFalse(process_group_exists(worker_pid))
            self.assertTrue(supervised.stdout.eof.is_set())
            self.assertTrue(supervised.stderr.eof.is_set())
        finally:
            force_group_cleanup(pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_a_blocking_graceful_callback_is_bounded_before_group_escalation(self) -> None:
        helper = """
import json, os, signal
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        def block_orderly_stop():
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            signal.pause()

        supervised = load_contract.SupervisedProcess(
            [sys.executable, "-c", helper],
            "responder",
            graceful=block_orderly_stop,
            stdout_limit=4096,
            stderr_limit=4096,
        )
        pgid = supervised.pgid
        worker_pid = supervised.orderly_stop_worker_pid
        try:
            supervised.wait_ready(timeout_ms=2_000)
            started = time.monotonic()
            with self.assertRaisesRegex(
                load_contract.ContractError, "orderly-stop callback exceeded"
            ):
                supervised.close(timeout_seconds=0.1)
            self.assertLess(time.monotonic() - started, 0.7)
            self.assertIsNotNone(supervised.process.returncode)
            self.assertFalse(process_group_exists(pgid))
            self.assertIsNotNone(worker_pid)
            self.assertFalse(process_group_exists(worker_pid))
            self.assertTrue(supervised.stdout.eof.is_set())
            self.assertTrue(supervised.stderr.eof.is_set())
        finally:
            force_group_cleanup(pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_orderly_stop_worker_terminates_and_joins_its_descendant_group(self) -> None:
        helper = """
import json, os, signal
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        with tempfile.TemporaryDirectory() as directory:
            pid_file = pathlib.Path(directory) / "orderly-descendant.pid"

            def spawn_blocked_descendant():
                ready_reader, ready_writer = os.pipe()
                child_pid = os.fork()
                if child_pid == 0:
                    os.close(ready_reader)
                    signal.signal(signal.SIGTERM, signal.SIG_IGN)
                    pid_file.write_text(str(os.getpid()), encoding="ascii")
                    os.write(ready_writer, b"ready")
                    os.close(ready_writer)
                    signal.pause()
                    os._exit(0)
                os.close(ready_writer)
                ready = os.read(ready_reader, 5)
                os.close(ready_reader)
                if ready != b"ready":
                    raise RuntimeError("orderly-stop descendant did not become ready")

            supervised = load_contract.SupervisedProcess(
                [sys.executable, "-c", helper],
                "responder",
                graceful=spawn_blocked_descendant,
                stdout_limit=4096,
                stderr_limit=4096,
            )
            endpoint_pgid = supervised.pgid
            worker_pgid = supervised.orderly_stop_worker_pid
            try:
                supervised.wait_ready(timeout_ms=2_000)
                supervised.close(timeout_seconds=0.1)
                self.assertTrue(pid_file.read_text(encoding="ascii"))
                self.assertIsNotNone(worker_pgid)
                self.assertFalse(process_group_exists(worker_pgid))
                self.assertFalse(process_group_exists(endpoint_pgid))
            finally:
                force_group_cleanup(endpoint_pgid)
                if worker_pgid is not None:
                    force_group_cleanup(worker_pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_term_handler_cleans_the_group_before_reporting_signal_exit(self) -> None:
        helper = """
import json, os, signal, subprocess, sys
subprocess.Popen([sys.executable, '-c', 'import signal; signal.pause()'])
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        old_handler = signal.getsignal(signal.SIGTERM)
        owner = load_contract.ProcessSupervisor(cleanup_wait_seconds=0.25)
        owner.__enter__()
        supervised = owner.start(
            [sys.executable, "-c", helper],
            "responder",
            stdout_limit=4096,
            stderr_limit=4096,
        )
        pgid = supervised.pgid
        try:
            supervised.wait_ready(timeout_ms=2_000)
            with self.assertRaises(SystemExit) as stopped:
                owner._on_signal(signal.SIGTERM, None)
            self.assertEqual(128 + signal.SIGTERM, stopped.exception.code)
            self.assertFalse(process_group_exists(pgid))
            self.assertEqual(old_handler, signal.getsignal(signal.SIGTERM))
        finally:
            try:
                owner.close()
            except load_contract.ContractError:
                pass
            force_group_cleanup(pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_signal_arriving_during_cleanup_is_deferred_until_group_exit(self) -> None:
        helper = """
import json, os, signal
signal.signal(signal.SIGTERM, signal.SIG_IGN)
print(json.dumps({
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096},
}), flush=True)
signal.pause()
"""
        owner = load_contract.ProcessSupervisor(cleanup_wait_seconds=0.25)
        owner.__enter__()
        supervised = owner.start(
            [sys.executable, "-c", helper],
            "responder",
            stdout_limit=4096,
            stderr_limit=4096,
        )
        pgid = supervised.pgid
        sender = threading.Timer(0.05, os.kill, args=(os.getpid(), signal.SIGTERM))
        try:
            supervised.wait_ready(timeout_ms=2_000)
            sender.start()
            with self.assertRaises(SystemExit) as stopped:
                owner.close()
            sender.join(timeout=1)
            self.assertEqual(128 + signal.SIGTERM, stopped.exception.code)
            self.assertFalse(process_group_exists(pgid))
            self.assertIsNotNone(supervised.process.returncode)
        finally:
            sender.cancel()
            sender.join(timeout=1)
            force_group_cleanup(pgid)
            if supervised.process.poll() is None:
                supervised.process.wait(timeout=2)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_oversized_readiness_is_rejected_without_retaining_the_line(self) -> None:
        helper = (
            "import os,signal; "
            f"os.write(1, b'x' * {load_contract.MAX_READY_BYTES + 65_536}); "
            "signal.pause()"
        )
        supervised = load_contract.SupervisedProcess(
            [sys.executable, "-c", helper],
            "responder",
            stdout_limit=load_contract.MAX_LOG_BYTES,
            stderr_limit=4096,
        )
        pgid = supervised.pgid
        try:
            with self.assertRaises(load_contract.ContractError):
                supervised.wait_ready(timeout_ms=2_000)
            self.assertLessEqual(
                supervised.stdout.readiness_retained_high_water,
                load_contract.MAX_READY_BYTES,
            )
            self.assertIsNotNone(supervised.process.returncode)
        finally:
            force_group_cleanup(pgid)

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_an_escaped_descendant_retaining_a_pipe_is_reported_and_bounded(self) -> None:
        child = (
            "import signal; "
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            "signal.pause()"
        )
        with tempfile.TemporaryDirectory() as directory:
            pid_file = pathlib.Path(directory) / "escaped.pid"
            helper = f"""
import json, os, pathlib, subprocess, sys
child = subprocess.Popen(
    [sys.executable, '-c', {child!r}],
    stdin=subprocess.DEVNULL,
    start_new_session=True,
)
pathlib.Path({str(pid_file)!r}).write_text(str(child.pid), encoding='ascii')
print(json.dumps({{
    'schema': 'sipx.comparative-load.ready.v1',
    'role': 'responder',
    'pid': os.getpid(),
    'address': '127.0.0.1:5060',
    'transport': 'udp',
    'limits': {{'active': 1, 'events': 1, 'stdout_bytes': 4096, 'stderr_bytes': 4096}},
}}), flush=True)
"""
            supervised = load_contract.SupervisedProcess(
                [sys.executable, "-c", helper],
                "responder",
                stdout_limit=4096,
                stderr_limit=4096,
            )
            escaped_pid = None
            try:
                supervised.wait_ready(timeout_ms=2_000)
                escaped_pid = int(pid_file.read_text(encoding="ascii"))
                with self.assertRaisesRegex(load_contract.ContractError, "retained.*pipe"):
                    supervised.close(timeout_seconds=0.1)
            finally:
                force_group_cleanup(supervised.pgid)
                if escaped_pid is not None:
                    try:
                        os.kill(escaped_pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    try:
                        supervised.close(timeout_seconds=0.5)
                    except load_contract.ContractError:
                        pass

    @unittest.skipUnless(os.name == "posix", "process groups require POSIX")
    def test_malformed_and_duplicate_readiness_fail_closed(self) -> None:
        malformed = load_contract.SupervisedProcess(
            [sys.executable, "-c", "print('{', flush=True); import signal; signal.pause()"],
            "responder",
            stdout_limit=4096,
            stderr_limit=4096,
        )
        with self.assertRaises(load_contract.ContractError):
            malformed.wait_ready(timeout_ms=2_000)
        self.assertIsNotNone(malformed.process.returncode)

        record = {
            "schema": load_contract.READY_SCHEMA,
            "role": "responder",
            "pid": 0,
            "address": "127.0.0.1:5060",
            "transport": "udp",
            "limits": {"active": 1, "events": 1, "stdout_bytes": 4096, "stderr_bytes": 4096},
        }
        helper = (
            "import json,os,signal; r="
            + repr(record)
            + "; r['pid']=os.getpid(); print(json.dumps(r),flush=True); "
              "print(json.dumps(r),flush=True); signal.pause()"
        )
        duplicate = load_contract.SupervisedProcess(
            [sys.executable, "-c", helper],
            "responder",
            stdout_limit=4096,
            stderr_limit=4096,
        )
        duplicate.wait_ready(timeout_ms=2_000)
        with self.assertRaises(load_contract.ContractError):
            duplicate.close(timeout_seconds=0.25)

        unterminated = load_contract.SupervisedProcess(
            [
                sys.executable,
                "-c",
                "import json,os; print(json.dumps({'schema':'sipx.comparative-load.ready.v1','role':'responder','pid':os.getpid(),'address':'127.0.0.1:5060','transport':'udp','limits':{'active':1,'events':1,'stdout_bytes':4096,'stderr_bytes':4096}}),end='',flush=True)",
            ],
            "responder",
            stdout_limit=4096,
            stderr_limit=4096,
        )
        with self.assertRaisesRegex(load_contract.ContractError, "line terminator"):
            unterminated.wait_ready(timeout_ms=2_000)

        invalid_driver = {
            "schema": load_contract.READY_SCHEMA,
            "role": "driver",
            "pid": 1,
            "address": None,
            "transport": "udp",
            "limits": {"active": 1, "events": 1, "stdout_bytes": 4096, "stderr_bytes": 4096},
        }
        with self.assertRaises(load_contract.ContractError):
            load_contract.validate_readiness(invalid_driver, "driver")


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
