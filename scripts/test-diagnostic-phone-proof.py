#!/usr/bin/env python3
"""Tests for the diagnostic-phone proof runner."""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "diagnostic-phone-proof.py"
SPEC = importlib.util.spec_from_file_location("diagnostic_phone_proof", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
proof = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = proof
SPEC.loader.exec_module(proof)


class MarkerDiscovery(unittest.TestCase):
    def test_attributes_between_marker_and_function_do_not_hide_a_vector(self):
        source = """\
/// `DPH-12`: device vector.
#[cfg(target_os = "linux")]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
)]
async fn virtual_device() {}
"""
        self.assertEqual(proof.marked_tests(source)[12], ["virtual_device"])

    def test_one_comment_can_attach_more_than_one_vector(self):
        source = """\
/// DPH-8 and DPH-9 share one bounded shell scenario.
#[tokio::test]
async fn headers_and_scenario() {}
"""
        marked = proof.marked_tests(source)
        self.assertEqual(marked[8], ["headers_and_scenario"])
        self.assertEqual(marked[9], ["headers_and_scenario"])


class PeerEvidence(unittest.TestCase):
    def test_missing_wss_test_cannot_be_inferred_from_tls_plus_websocket(self):
        source = """\
async fn registers_against_a_real_server_over_tls() {}
async fn registers_against_a_real_server_over_websocket() {}
"""
        coverage = proof.interop_coverage(source)
        self.assertEqual(coverage["wss"], ())

    def test_current_profiles_are_two_paths_for_every_claimed_transport(self):
        source = proof.INTEROP_TESTS.read_text()
        coverage = proof.interop_coverage(source)
        for transport in ("udp", "tcp", "tls", "ws", "wss"):
            self.assertEqual(len(coverage[transport]), 2, transport)


class Bounds(unittest.TestCase):
    @mock.patch.object(proof.subprocess, "run")
    def test_every_spawn_has_a_finite_timeout(self, run: mock.Mock):
        run.return_value = subprocess.CompletedProcess(["cargo"], 0)
        result = proof.run_command(["cargo", "test"], 17)
        self.assertEqual(result.status, "passed")
        self.assertEqual(run.call_args.kwargs["timeout"], 17)

    @mock.patch.object(proof.subprocess, "run")
    def test_timeout_is_a_failure_not_a_skip(self, run: mock.Mock):
        run.side_effect = subprocess.TimeoutExpired(["cargo", "test"], 17)
        result = proof.run_command(["cargo", "test"], 17)
        self.assertEqual(result.status, "failed")
        self.assertIn("17s", result.detail)


class Matrix(unittest.TestCase):
    def test_the_contract_is_exactly_dph_1_through_12(self):
        self.assertEqual([vector.number for vector in proof.VECTORS], list(range(1, 13)))

    def test_open_evidence_makes_the_proof_fail(self):
        vectors = {
            vector.number: proof.CommandResult("present", "test") for vector in proof.VECTORS
        }
        coverage = {transport: ("one", "two") for transport in proof.TRANSPORT_TESTS}
        self.assertFalse(proof.failed(vectors, coverage))
        coverage["wss"] = ()
        self.assertTrue(proof.failed(vectors, coverage))

    def test_every_product_path_names_executable_phone_evidence(self):
        vectors = {
            vector.number: proof.CommandResult("present", "test") for vector in proof.VECTORS
        }
        products = proof.execute_product_paths(vectors, run=False, execute_checks=False)
        self.assertEqual(products["Opus"].status, "present")
        self.assertEqual(products["early media"].status, "present")
        self.assertEqual(products["authenticated INVITE"].status, "present")
        self.assertEqual(products["CLI reference"].status, "present")


if __name__ == "__main__":
    unittest.main()
