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

import argparse
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


def load_comparative_runner_module():
    """Import the execution runner while keeping the scripts directory off the import path."""
    spec = importlib.util.spec_from_file_location(
        "comparative_load_runner", ROOT / "scripts" / "comparative-load-run.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


load_runner = load_comparative_runner_module()


def load_comparative_driver_module():
    """Import the neutral traffic driver whose response accounting is contractual."""
    spec = importlib.util.spec_from_file_location(
        "comparative_load_driver", ROOT / "scripts" / "comparative-load-driver.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


load_driver = load_comparative_driver_module()

#: The reserved fixture identity. Assertions filter on it so a fixture's problem can never be
#: confused with a problem the real dataset has.
FIXTURE_STACK = "zz-fixture-stack"
FIXTURE_DIMENSION = "zz-fixture-dimension"
FIXTURE_REVISION = "0123456789abcdef0123456789abcdef01234567"
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
        "capability_inventory": True,
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
                "url": f"https://example.invalid/source/blob/{FIXTURE_REVISION}/message.rs",
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

    def test_an_exact_id_inventory_without_its_ledger_is_rejected(self) -> None:
        problems = report.capability_problems(
            [],
            [a_stack()],
            TODAY,
            expectations={FIXTURE_STACK: (FIXTURE_REVISION, {"zz-capability"})},
        )
        self.assertTrue(any("no corresponding ledger" in problem for problem in problems), problems)

    def test_deleting_both_inventory_files_is_rejected_by_the_stack_anchor(self) -> None:
        problems = report.capability_problems([], [a_stack()], TODAY, expectations={})
        for phrase in ("requires a capability ledger", "requires an exact-ID inventory"):
            self.assertTrue(any(phrase in problem for problem in problems), problems)

    def test_removing_every_stack_anchor_is_rejected(self) -> None:
        stack = a_stack()
        del stack["capability_inventory"]
        problems = report.capability_problems([], [stack], TODAY, expectations={})
        self.assertTrue(any("no comparison stack requires" in problem for problem in problems), problems)

    def test_an_unevidenced_leaf_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(evidence=[]))
        self.assertTrue(any("cites no evidence" in problem for problem in problems), problems)

    def test_an_implemented_leaf_without_rust_source_is_rejected(self) -> None:
        problems = capability_problems_for(a_capability(implementation=[]))
        self.assertTrue(any("Rust source evidence" in problem for problem in problems), problems)

    def test_implementation_evidence_must_be_workspace_rust_source(self) -> None:
        problems = capability_problems_for(a_capability(implementation=["README.md"]))
        self.assertTrue(any("workspace crate" in problem for problem in problems), problems)

    def test_implementation_evidence_cannot_escape_the_crates_directory(self) -> None:
        problems = capability_problems_for(
            a_capability(implementation=["crates/../fuzz/fuzz_targets/parse_stream.rs"])
        )
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

    def test_measured_confidence_rejects_a_revision_hidden_in_a_mutable_url(self) -> None:
        problems = capability_problems_for(
            a_capability(
                evidence=[
                    {
                        "url": f"https://example.invalid/main?claimed_revision={FIXTURE_REVISION}",
                        "note": "a mutable branch with a decorative revision",
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

    def test_non_scalar_dispositions_are_refused_without_a_crash(self) -> None:
        for field in ("confidence", "ownership", "status"):
            for value in (["not", "scalar"], {"not": "scalar"}):
                with self.subTest(field=field, value=value):
                    capability = a_capability(**{field: value})
                    ledger = a_capability_ledger([capability])
                    schema = report.capability_schema_problems(ledger)
                    self.assertTrue(any(f"invalid {field}" in p for p in schema), schema)
                    substantive = report.capability_problems(
                        [ledger],
                        [a_stack()],
                        TODAY,
                        expectations=expectations_for([ledger]),
                    )
                    self.assertTrue(substantive)

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
            return report.external_story_index_problems(
                directory,
                lambda _repository, _revision, paths: {path: "b" * 40 for path in paths},
            )

    def test_a_complete_pinned_index_is_accepted_and_derives_the_url(self) -> None:
        value = self.valid_index()
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            (directory / "cluster.json").write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(
                [],
                report.external_story_index_problems(
                    directory,
                    lambda _repository, _revision, paths: {
                        path: "b" * 40 for path in paths
                    },
                ),
            )
            self.assertEqual(
                {
                    "https://example.invalid/cluster/blob/"
                    f"{'a' * 40}/docs/stories/ZZ-1-a-story.md"
                },
                report.external_story_urls(directory),
            )

    def test_a_well_formed_but_wrong_blob_identity_is_rejected(self) -> None:
        value = self.valid_index()
        value["stories"][0]["blob_sha"] = "0" * 40
        problems = self.problems_for(value)
        self.assertTrue(any("pinned commit carries" in problem for problem in problems), problems)

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

    def test_an_object_valued_capability_id_is_refused_without_a_crash(self) -> None:
        value = self.valid_inventory()
        value["expected_ids"] = [{"not": "hashable"}]
        expectations, problems = self.load(value)
        self.assertEqual({}, expectations)
        self.assertTrue(any("invalid capability" in problem for problem in problems), problems)


class MalformedCapabilityFiles(unittest.TestCase):
    def test_an_array_ledger_is_loaded_for_a_typed_refusal_instead_of_crashing(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            directory = pathlib.Path(raw)
            (directory / "broken.json").write_text("[]", encoding="utf-8")
            original = report.CAPABILITIES
            try:
                report.CAPABILITIES = directory
                ledgers = report.capability_ledgers()
            finally:
                report.CAPABILITIES = original
        self.assertEqual([[]], ledgers)
        self.assertEqual(
            ["capability ledger is not an object"],
            report.capability_schema_problems(ledgers[0]),
        )


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

    def test_a_capability_inventory_marker_must_be_boolean(self) -> None:
        problems = report.schema_problems("stack", a_stack(capability_inventory="yes"))
        self.assertTrue(any("non-boolean" in problem for problem in problems), problems)


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


def a_load_environment(manifest):
    """The immutable execution inventory X-99 records beside each run's manifest."""
    builds = []
    for build in manifest["builds"]:
        builds.append(
            {
                "endpoint_id": build["endpoint_id"],
                "role": build["role"],
                "revision": build["revision"],
                "artifact": f"/opt/{build['endpoint_id']}",
                "artifact_sha256": build["artifact_sha256"],
                "build_command": "make release",
                "toolchain": "fixture-compiler 1.0.0",
                "features": [],
                "dependencies": [],
            }
        )
    return {
        "schema": "sipx.comparative-load.environment.v1",
        "captured_utc": "2026-08-05T11:00:00Z",
        "host": {
            "os": "fixture-os",
            "kernel": "6.0.0-fixture",
            "architecture": "fixture-arch",
            "logical_cpus": 8,
            "memory_bytes": 8 * 1024 * 1024 * 1024,
            "cpu_governor": "performance",
            "clock": "monotonic",
        },
        "socket_limits": {
            "rlimit_nofile_soft": 1024,
            "rlimit_nofile_hard": 4096,
            "rmem_max": 212_992,
            "wmem_max": 212_992,
            "rmem_default": 212_992,
            "wmem_default": 212_992,
        },
        "toolchains": [{"tool": "fixture-compiler", "version": "1.0.0"}],
        "builds": builds,
        "commands": ["comparative-load-run --plan fixture"],
        "seed": manifest["seed"],
        "contract_sha256": load_contract.contract_hash(),
    }


def a_load_preflight(phase="preflight", dialogs=20):
    """Correctness evidence gathered before any capacity work."""
    return {
        "schema": "sipx.comparative-load.preflight.v1",
        "phase": phase,
        "rate_per_second": 1,
        "dialogs": dialogs,
        "offered": dialogs,
        "completed": dialogs,
        "five_steps_observed": True,
        "post_drain_zero": True,
        "passed": True,
        "started_utc": "2026-08-05T11:05:00Z",
        "elapsed_ms": dialogs * 1000 + 500,
    }


def a_load_headroom(manifest):
    """The driver's proof that it is not the bottleneck at twice the tested ceiling."""
    rate = 2 * manifest["ceiling"]
    offered = rate * 60
    return {
        "schema": "sipx.comparative-load.headroom.v1",
        "fixture": "packaged-minimal-fixture",
        "rate_per_second": rate,
        "offered": offered,
        "completed": offered,
        "completion_ratio": 1.0,
        "setup_p99_ms": 4,
        "driver_cpu_fraction": 0.21,
        "passed": True,
        "started_utc": "2026-08-05T11:07:00Z",
        "elapsed_ms": 70_500,
    }


def a_rate_result(manifest, rate_index, repetition, passed=True):
    """One ladder repetition consistent with the manifest's derived seed and rate."""
    rate = load_contract.ladder_rates(manifest["ceiling"])[rate_index]
    offered = rate * 60
    missed = 0 if passed else max(offered // 100, 1)
    completed = offered - missed
    result = a_load_result(manifest)
    result["status"] = "passed" if passed else "failed"
    result["run"].update(
        {
            "rate_index": rate_index,
            "rate_per_second": rate,
            "repetition": repetition,
            "seed": manifest["seed"]
            ^ (manifest["direction"]["index"] << 56)
            ^ (rate_index << 32)
            ^ repetition,
        }
    )
    result["counts"].update(
        {"offered": offered, "established": completed, "completed": completed}
    )
    result["responses"] = {
        "provisional": {"100": offered}
        if manifest["provisional_policy"] == "trying_100" and passed
        else ({"100": completed} if manifest["provisional_policy"] == "trying_100" else {}),
        "final": {"200": completed * 2},
    }
    result["errors"] = {
        name: 0 for name in load_contract.TERMINAL_ERRORS + load_contract.RUN_ERRORS
    }
    result["errors"]["transaction_timeout"] = missed
    result["latency_ms"]["setup"]["count"] = completed
    result["latency_ms"]["teardown"]["count"] = completed
    return result


def a_full_ladder(manifest, failed_rates=()):
    """Five repetitions for every attempted rate, stopping after two consecutive failures."""
    results = {}
    consecutive = 0
    for rate_index in range(len(load_contract.LADDER_DIVISORS)):
        if consecutive >= 2:
            break
        passed = rate_index not in failed_rates
        for repetition in range(load_contract.REPETITIONS):
            results[(rate_index, repetition)] = a_rate_result(
                manifest, rate_index, repetition, passed=passed
            )
        consecutive = consecutive + 1 if not passed else 0
    return results


def a_load_run(manifest=None, results=None, omitted=None):
    """One complete measured run directory, loaded the way the checker reads it."""
    manifest = manifest or a_load_manifest()
    if results is None:
        results = a_full_ladder(manifest)
    return {
        "manifest": manifest,
        "environment": a_load_environment(manifest),
        "preflight": a_load_preflight(),
        "qualification": a_load_preflight(phase="qualification", dialogs=100),
        "headroom": a_load_headroom(manifest),
        "results": results,
        "omissions": {
            "schema": "sipx.comparative-load.omissions.v1",
            "omitted": list(omitted or ()),
        },
    }


class TheComparativeLoadDriver(unittest.TestCase):
    """The measuring instrument distinguishes validated evidence from wire failures."""

    def test_an_invalid_success_response_is_not_counted_as_validated_evidence(self) -> None:
        args = argparse.Namespace(
            seed=7,
            run_id="0123456789abcdef0123456789abcdef",
            rate=1,
            max_active=1,
            provisional="trying_100",
            local="127.0.0.1:0",
            target="127.0.0.1:9",
        )
        driver = load_driver.Driver(args)
        try:
            driver.counting = True
            driver.offer(0)
            dialog = driver.dialogs[0]
            ids = dialog["ids"]
            response = (
                "SIP/2.0 200 OK\r\n"
                f"Via: SIP/2.0/UDP {driver.local};rport;branch={ids['invite_branch']}\r\n"
                f"From: <sip:driver@{driver.local}>;tag={ids['from_tag']}\r\n"
                f"To: <sip:load@{driver.target_text}>;tag={ids['to_tag']}\r\n"
                f"Call-ID: {ids['call_id']}\r\n"
                "CSeq: 1 INVITE\r\n"
                f"Contact: <sip:load@{driver.target_text}>\r\n"
                "Content-Length: 0\r\n\r\n"
            ).encode()

            # The required provisional response was never observed. The successful-coded
            # datagram is therefore failure evidence, not a validated transaction response.
            driver.handle_datagram(response)

            self.assertEqual({}, driver.responses["final"])
            self.assertEqual(1, driver.errors["invalid_message"])
            self.assertEqual(0, driver.counts["established"])
            self.assertEqual(0, driver.counts["completed"])
        finally:
            driver.sock.close()


class TheComparativeLoadRunner(unittest.TestCase):
    """The measuring ends agree on one explicit provisional-response policy."""

    def test_the_runner_requests_one_trying_from_every_dialog(self) -> None:
        execution = load_runner.Execution(
            argparse.Namespace(
                endpoint=ROOT / "docs" / "comparison" / "load" / "endpoints" / "sipx.json",
                driver=ROOT
                / "docs"
                / "comparison"
                / "load"
                / "endpoints"
                / "profile-driver.json",
                ceiling=1024,
                seed=7,
                run_id="0123456789abcdef0123456789abcdef",
                out=None,
                phases=None,
                direction_index=0,
                resume=False,
            )
        )
        self.assertEqual("trying_100", load_runner.PROVISIONAL_POLICY)
        driver = execution.driver_template()
        self.assertEqual(
            load_runner.PROVISIONAL_POLICY, driver[driver.index("--provisional") + 1]
        )
        fixture = execution.fixture_template()
        self.assertEqual(
            load_runner.PROVISIONAL_POLICY, fixture[fixture.index("--provisional") + 1]
        )
        responder = execution.responder_template()
        self.assertEqual("100", responder[responder.index("--provisional-percent") + 1])


LOAD_RUN_KEY = "runs/0123456789abcdef0123456789abcdef"


def a_load_dataset():
    """The published dataset: measured directions, disclosed omissions, explicit scope."""
    return {
        "schema": "sipx.comparative-load.dataset.v1",
        "evaluated_at": TODAY.isoformat(),
        "driver": {
            "id": "endpoint-a",
            "revision": "revision-a",
            "artifact_sha256": "a" * 64,
        },
        "endpoints": [
            {
                "id": "endpoint-b",
                "as_responder": {"status": "measured", "run": LOAD_RUN_KEY},
                "as_driver": {
                    "status": "not_measured",
                    "reason": "the pinned build ships no neutral-profile driver",
                },
                "internal_state": {
                    "visibility": "endpoint-reported",
                    "note": "post-drain state is read from the endpoint's own summary",
                },
            }
        ],
        "scope": {
            "workload": "UDP dialog signalling without SDP or media",
            "not_inferred": ["secure transports", "connection churn", "audio"],
        },
    }


def a_second_load_run(endpoint_id="endpoint-c", run_id="fedcba9876543210fedcba9876543210", **kwargs):
    """A second measured endpoint, for cross-endpoint interval tests."""
    manifest = a_load_manifest()
    manifest["run_id"] = run_id
    manifest["direction"]["responder"] = endpoint_id
    manifest["builds"][1]["endpoint_id"] = endpoint_id
    manifest["builds"][1]["revision"] = "revision-c"
    manifest["builds"][1]["artifact_sha256"] = "c" * 64
    return manifest, a_load_run(manifest, **kwargs)


def load_dataset_problems(dataset=None, runs=None, stacks=None, today=TODAY):
    dataset = dataset if dataset is not None else a_load_dataset()
    if runs is None:
        runs = {LOAD_RUN_KEY: a_load_run()}
    stack_list = stacks if stacks is not None else [a_stack(id="endpoint-b", name="Fixture B")]
    return report.load_problems(dataset, runs, stack_list, today)


class TheComparativeLoadDataset(unittest.TestCase):
    """X-99's published result: fresh, hash-pinned, qualified first, and never a ranking."""

    def comparable_pair(self, change_manifest=lambda manifest: None):
        dataset = a_load_dataset()
        manifest = a_load_manifest()
        manifest["run_id"] = "fedcba9876543210fedcba9876543210"
        manifest["direction"]["responder"] = "endpoint-c"
        manifest["builds"][1]["endpoint_id"] = "endpoint-c"
        manifest["builds"][1]["revision"] = "revision-c"
        manifest["builds"][1]["artifact_sha256"] = "c" * 64
        change_manifest(manifest)
        key = "runs/" + manifest["run_id"]
        dataset["endpoints"].append(
            {
                "id": "endpoint-c",
                "as_responder": {"status": "measured", "run": key},
                "as_driver": {
                    "status": "not_measured",
                    "reason": "the pinned build ships no neutral-profile driver",
                },
                "internal_state": {
                    "visibility": "harness-observed",
                    "note": "post-drain state is observed by the harness only",
                },
            }
        )
        runs = {LOAD_RUN_KEY: a_load_run(), key: a_load_run(manifest)}
        stacks = [
            a_stack(id="endpoint-b", name="Fixture B"),
            a_stack(id="endpoint-c", name="Fixture C"),
        ]
        return dataset, runs, stacks

    def test_a_complete_dataset_has_no_problems(self) -> None:
        self.assertEqual([], load_dataset_problems())

    def test_a_stale_dataset_names_the_refresh_command(self) -> None:
        dataset = a_load_dataset()
        dataset["evaluated_at"] = (
            TODAY - datetime.timedelta(days=report.MAX_OBSERVATION_AGE_DAYS + 1)
        ).isoformat()
        problems = load_dataset_problems(dataset)
        self.assertTrue(any("stale" in p for p in problems), problems)
        self.assertTrue(any(report.LOAD_REFRESH_COMMAND in p for p in problems), problems)

    def test_an_artifact_hash_disagreement_is_refused(self) -> None:
        run = a_load_run()
        run["environment"]["builds"][1]["artifact_sha256"] = "d" * 64
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(
            any("artifact" in p and "manifest" in p for p in problems), problems
        )

    def test_cross_endpoint_results_must_share_the_same_host(self) -> None:
        dataset, runs, stacks = self.comparable_pair(
            lambda manifest: manifest["machine"].update({"logical_cpus": 16})
        )
        problems = report.load_problems(dataset, runs, stacks, TODAY)
        self.assertTrue(any("same host" in p for p in problems), problems)

    def test_cross_endpoint_results_must_share_the_execution_profile(self) -> None:
        changes = (
            ("ceiling", lambda manifest: manifest.update({"ceiling": 2048})),
            ("seed", lambda manifest: manifest.update({"seed": 19})),
            (
                "provisional-response policy",
                lambda manifest: manifest.update({"provisional_policy": "none"}),
            ),
        )
        for expected, change in changes:
            with self.subTest(expected=expected):
                dataset, runs, stacks = self.comparable_pair(change)
                problems = report.load_problems(dataset, runs, stacks, TODAY)
                self.assertTrue(any(expected in p for p in problems), problems)

    def test_contract_drift_is_refused(self) -> None:
        run = a_load_run()
        run["environment"]["contract_sha256"] = "e" * 64
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(any("contract" in p for p in problems), problems)

    def test_a_failed_qualification_cannot_carry_measurements(self) -> None:
        run = a_load_run()
        run["qualification"]["completed"] = 40
        run["qualification"]["passed"] = False
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(
            any("correctness prerequisite failed" in p for p in problems), problems
        )

    def test_a_qualification_below_one_hundred_dialogs_is_refused(self) -> None:
        run = a_load_run()
        run["qualification"]["dialogs"] = 60
        run["qualification"]["offered"] = 60
        run["qualification"]["completed"] = 60
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(any("one hundred" in p or "100" in p for p in problems), problems)

    def test_a_missing_direction_needs_a_reason(self) -> None:
        dataset = a_load_dataset()
        dataset["endpoints"][0]["as_driver"]["reason"] = ""
        problems = load_dataset_problems(dataset)
        self.assertTrue(any("reason" in p for p in problems), problems)

    def test_a_disclosed_direction_cannot_smuggle_a_run(self) -> None:
        dataset = a_load_dataset()
        dataset["endpoints"][0]["as_driver"]["run"] = LOAD_RUN_KEY
        problems = load_dataset_problems(dataset)
        self.assertTrue(any("not_measured" in p for p in problems), problems)

    def test_headroom_below_twice_the_ceiling_is_refused(self) -> None:
        run = a_load_run()
        run["headroom"]["rate_per_second"] = run["manifest"]["ceiling"]
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(any("twice" in p for p in problems), problems)

    def test_a_hot_driver_invalidates_the_execution(self) -> None:
        run = a_load_run()
        run["headroom"]["driver_cpu_fraction"] = 0.93
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(any("cpu" in p.lower() for p in problems), problems)

    def test_an_undeclared_endpoint_is_refused(self) -> None:
        problems = load_dataset_problems(stacks=[])
        self.assertTrue(any("stacks.json" in p for p in problems), problems)

    def test_an_inconsistent_omission_is_refused(self) -> None:
        run = a_load_run(omitted=[{"rate_index": 5, "rate_per_second": 1024, "reason": "two_consecutive_failed_rates"}])
        problems = load_dataset_problems(runs={LOAD_RUN_KEY: run})
        self.assertTrue(any("omission" in p or "omitted" in p for p in problems), problems)

    def test_omitted_rates_render_as_not_run_rather_than_zero(self) -> None:
        manifest = a_load_manifest()
        results = a_full_ladder(manifest, failed_rates={3, 4})
        rates = load_contract.ladder_rates(manifest["ceiling"])
        run = a_load_run(
            manifest,
            results=results,
            omitted=[
                {
                    "rate_index": 5,
                    "rate_per_second": rates[5],
                    "reason": "two_consecutive_failed_rates",
                }
            ],
        )
        runs = {LOAD_RUN_KEY: run}
        self.assertEqual([], load_dataset_problems(runs=runs))
        text = "\n".join(
            report.render_load_section(
                a_load_dataset(), runs, [a_stack(id="endpoint-b", name="Fixture B")]
            )
        )
        self.assertIn("not run: two consecutive rates failed", text)
        self.assertNotIn("| 0/s |", text)

    def test_overlapping_intervals_are_labelled_inconclusive(self) -> None:
        dataset = a_load_dataset()
        manifest_c, run_c = a_second_load_run()
        dataset["endpoints"].append(
            {
                "id": "endpoint-c",
                "as_responder": {"status": "measured", "run": "runs/" + manifest_c["run_id"]},
                "as_driver": {
                    "status": "not_measured",
                    "reason": "the pinned build ships no neutral-profile driver",
                },
                "internal_state": {
                    "visibility": "harness-observed",
                    "note": "post-drain state is observed by the harness only",
                },
            }
        )
        runs = {LOAD_RUN_KEY: a_load_run(), "runs/" + manifest_c["run_id"]: run_c}
        stacks = [
            a_stack(id="endpoint-b", name="Fixture B"),
            a_stack(id="endpoint-c", name="Fixture C"),
        ]
        self.assertEqual([], report.load_problems(dataset, runs, stacks, TODAY))
        text = "\n".join(report.render_load_section(dataset, runs, stacks))
        self.assertIn("inconclusive", text)
        self.assertNotIn("winner", text.lower())

    def test_distinct_intervals_still_never_claim_a_winner(self) -> None:
        dataset = a_load_dataset()
        manifest_c, run_c = a_second_load_run(
            **{"results": None}
        )
        run_c["results"] = a_full_ladder(manifest_c, failed_rates={4, 5})
        dataset["endpoints"].append(
            {
                "id": "endpoint-c",
                "as_responder": {"status": "measured", "run": "runs/" + manifest_c["run_id"]},
                "as_driver": {
                    "status": "not_measured",
                    "reason": "the pinned build ships no neutral-profile driver",
                },
                "internal_state": {
                    "visibility": "harness-observed",
                    "note": "post-drain state is observed by the harness only",
                },
            }
        )
        runs = {LOAD_RUN_KEY: a_load_run(), "runs/" + manifest_c["run_id"]: run_c}
        stacks = [
            a_stack(id="endpoint-b", name="Fixture B"),
            a_stack(id="endpoint-c", name="Fixture C"),
        ]
        text = "\n".join(report.render_load_section(dataset, runs, stacks))
        self.assertIn("higher on this machine", text)
        self.assertNotIn("winner", text.lower())
        self.assertIn("not a general ranking", text)

    def test_the_scope_limitation_is_rendered(self) -> None:
        runs = {LOAD_RUN_KEY: a_load_run()}
        text = "\n".join(
            report.render_load_section(
                a_load_dataset(), runs, [a_stack(id="endpoint-b", name="Fixture B")]
            )
        )
        self.assertIn("UDP dialog signalling without SDP or media", text)
        self.assertIn("not inferred", text)

    def test_the_harness_capabilities_and_proxy_boundary_are_rendered(self) -> None:
        dataset = a_load_dataset()
        dataset["scope"]["not_inferred"].append(
            "proxy, registrar, routing or cluster behavior; those workloads belong to sipx.clstr"
        )
        text = "\n".join(
            report.render_load_section(
                dataset,
                {LOAD_RUN_KEY: a_load_run()},
                [a_stack(id="endpoint-b", name="Fixture B")],
            )
        )
        for capability in (
            "fixed open-loop offered load",
            "correctness qualification",
            "driver headroom",
            "six rates",
            "five repetitions",
            "setup and teardown latency",
            "process resource samples",
            "bounded cleanup",
            "raw evidence",
        ):
            self.assertIn(capability, text)
        self.assertIn("sipx.clstr", text)

    def test_a_supported_top_rate_is_rendered_as_a_lower_bound(self) -> None:
        text = "\n".join(
            report.render_load_section(
                a_load_dataset(),
                {LOAD_RUN_KEY: a_load_run()},
                [a_stack(id="endpoint-b", name="Fixture B")],
            )
        )
        self.assertIn("at least **1024 calls/s**", text)
        self.assertIn("highest tested rate", text)


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

    def test_the_load_dataset_has_no_outstanding_problems(self) -> None:
        dataset = report.load_dataset()
        self.assertIsNotNone(dataset, "docs/comparison/load/dataset.json is X-99 evidence")
        _, stacks, _ = report.dataset()
        runs = report.load_runs(dataset)
        self.assertEqual(
            [], report.load_problems(dataset, runs, stacks, datetime.date.today())
        )

    def test_the_report_is_current(self) -> None:
        dimensions, stacks, observations = report.dataset()
        load = report.load_dataset()
        rendered = report.render(
            dimensions,
            stacks,
            observations,
            report.generated_values(),
            report.capability_ledgers(),
            load=(load, report.load_runs(load)),
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
