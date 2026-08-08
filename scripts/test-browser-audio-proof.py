#!/usr/bin/env python3
"""Adversarial self-test for the M-51 browser-audio proof harness."""

from __future__ import annotations

import contextlib
import copy
import importlib.util
import inspect
import json
import os
import pathlib
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from typing import Any, NamedTuple


ROOT = pathlib.Path(__file__).resolve().parents[1]
DRIVER_PATH = ROOT / "tests/browser-audio/driver.py"
RUNNER = ROOT / "tests/browser-audio/run.sh"
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("browser_audio_driver", DRIVER_PATH)
assert SPEC is not None and SPEC.loader is not None
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)


PIN = "ERERERERERERERERERERERERERERERERERERERERERE="


def executable(path: pathlib.Path, body: str) -> pathlib.Path:
    path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


def browser(role: str) -> dict:
    return {
        "contract": DRIVER.CONTRACT,
        "type": "proof.result",
        "role": role,
        "codec": {"mime_type": "audio/opus", "payload_type": 111, "clock_rate": 48000},
        "security": {
            "wss_spki_sha256": PIN,
            "dtls_state": "connected",
            "setup_role": "active" if role == "browser-offerer" else "passive",
            "dtls_cipher": "cipher",
            # The profiles a real run of this harness negotiates, per role. The browser picks
            # when it is the DTLS server, which is the offerer role, and picks counter mode;
            # sipx picks in the answerer role and picks its strongest, which is AEAD-GCM. That
            # asymmetry is what makes the answerer role M-72's key-derivation witness.
            "srtp_profile": (
                "AES_CM_128_HMAC_SHA1_80" if role == "browser-offerer" else "AEAD_AES_256_GCM"
            ),
        },
        "peer": {
            "browser_name": "chrome",
            "browser_version": "150.0.7871.46",
            "driver_version": "150.0.7871.46",
        },
        "candidate_pair": {
            "id": "pair-1",
            "selected": True,
            "nominated": True,
            "state": "succeeded",
            "component": 1,
            "local": {"candidate_type": "host", "address": "192.0.2.10", "port": 40000},
            "remote": {"candidate_type": "srflx", "address": "198.51.100.20", "port": 41000},
        },
        "media": {
            "inbound_packets": 4,
            "outbound_packets": 5,
            "inbound_bytes": 640,
            "outbound_bytes": 800,
            "received_audio_energy": 0.5,
            "oscillator_frames": 960,
        },
        "sip": {"order": ["invite", "final", "ack", "bye", "bye-final"]},
    }


def sipx(role: str) -> dict:
    return {
        "status": "answered",
        "media_profile": "browser-audio",
        "negotiated_codec": "opus",
        "negotiated_payload_type": 111,
        "negotiated_clock_rate": 48000,
        "negotiated_keying": "dtls-srtp",
        "browser_role": role,
        "ice_component": 1,
        "nominated_local": "198.51.100.20:41000",
        "nominated_remote": "192.0.2.10:40000",
        "media_state": "running",
        "packets_sent": 5,
        "packets_received": 4,
        "received_audio_peak": 12000,
    }


def unused_rtcp_candidate_browser() -> dict:
    result = browser("browser-offerer")
    result["sdp"] = {
        "local": {
            "candidate_components": ["1", "2"],
            "raw": "a=candidate:left 1 UDP 100 192.0.2.10 40000 typ host\r\n"
            "a=candidate:left 2 UDP 99 192.0.2.10 40001 typ host\r\n",
        },
        "remote": {
            "candidate_components": ["1"],
            "raw": "a=candidate:right 1 UDP 100 198.51.100.20 41000 typ host\r\n",
        },
    }
    return result


def negative(name: str, role: str, digest: str) -> dict:
    facts = {"rtp_packets": 0, "fallback_attempted": False}
    if name == "FingerprintMismatch":
        facts.update({"selected_pair": True, "nominated": True, "dtls_state": "failed"})
    elif name == "NoNominatedPair":
        facts.update({"ice_started": True, "ice_state": "closed", "selected_pair": False, "nominated": False, "dtls_state": "closed"})
    else:
        facts.update({"ice_started": False, "dtls_state": "not-started"})
    browser_result = {
        "contract": DRIVER.CONTRACT,
        "type": "proof.negative-browser",
        "role": role,
        "mutation": name,
        "error": f"observed {name}",
        "facts": facts,
    }
    return {
        "positive_role": role,
        "positive_sha256": digest,
        "browser": browser_result,
        "sipx": {"error": name},
    }


@contextlib.contextmanager
def validator_blind_to(messages: tuple[str, ...]):
    """Run the block against a validator that ignores exactly the named refusals.

    This is the stub every non-vacuity claim in this suite is measured against. A test that
    mutates a fact and asserts a refusal proves nothing unless the refusal came from the check
    it names, so each such test is re-run here with that check — and only that check — removed.
    If the test still passes blind, the mutation was being refused by something else and the
    assertion was reading as coverage without buying any.
    """
    ignored = frozenset(messages)
    original = DRIVER.require

    def blind(condition: bool, message: str) -> None:
        if message in ignored:
            return
        original(condition, message)

    DRIVER.require = blind
    try:
        yield
    finally:
        DRIVER.require = original


#: Each fact `test_every_positive_fact_is_asserted` mutates, the value it mutates it to, and the
#: exact refusals the validator owes that fact. The messages are what `validator_blind_to` removes
#: to prove the assertion is load-bearing.
POSITIVE_FACTS: tuple[tuple[str, str, Any, tuple[str, ...]], ...] = (
    ("codec", "mime_type", "audio/PCMU", ("selected codec is not Opus",)),
    ("security", "dtls_state", "connecting", ("DTLS is not connected",)),
    (
        "security",
        "srtp_profile",
        "",
        ("the negotiated SRTP profile is not a name the registry carries",),
    ),
    ("candidate_pair", "nominated", False, ("candidate pair is not nominated",)),
    ("candidate_pair", "component", 2, ("browser-audio selected another ICE component",)),
    ("media", "inbound_packets", 0, ("inbound_packets must be positive",)),
    ("media", "received_audio_energy", 0, ("received audio energy must be positive",)),
    (
        "sip",
        "order",
        ["invite", "final", "ack", "bye"],
        ("SIP lifecycle evidence is incomplete",),
    ),
    (
        "sip",
        "order",
        ["invite", "ack", "final", "bye", "bye-final"],
        ("SIP lifecycle is out of order",),
    ),
)


#: How a test is recognised as making a claim about the validator. Named here rather than written
#: inline, so that the coverage test quoting them does not make the coverage test itself look like
#: one of the assertions it is counting.
VALIDATOR_PROBES = ("self.assert_refused(", "DRIVER.validate_proof(")


class Audited(NamedTuple):
    """One assertion this suite makes about `validate_proof`, and how it is shown to be load-bearing.

    `refusals` are the exact messages `validator_blind_to` removes. `blind` is what the named test
    must then do: `AssertionError` — the default and the clean case — means the blind validator
    accepted the mutated evidence, so the removed check was the only thing refusing it. Any other
    exception records that removing the check leaves the validator unable to finish at all, which
    is a weaker shape of the same conclusion and is spelled out where it applies.

    A row with no `refusals` records an assertion no stub can reach, and `claim` says why. An empty
    row is a judgement on the record rather than an omission.
    """

    test: str
    claim: str
    refusals: tuple[str, ...] = ()
    blind: type[BaseException] = AssertionError


#: The audit. `test_every_proof_assertion_is_non_vacuous` walks it, and
#: `test_the_audit_covers_every_assertion_about_the_validator` refuses to let a new assertion about
#: the validator skip the audit by omission.
PROOF_ASSERTIONS: tuple[Audited, ...] = (
    Audited(
        "test_complete_two_role_proof_is_accepted",
        "complete evidence is accepted, and the result names both roles; asserts a result rather "
        "than a refusal, so no removed check can make it pass",
    ),
    Audited(
        "test_unused_candidate_compatibility_evidence_is_mandatory_and_exact",
        "the compatibility offer carries exactly components 1 and 2",
        ("compatibility offer did not contain exactly components 1 and 2",),
    ),
    Audited(
        "test_one_role_cannot_be_called_complete",
        "a missing role is not a complete proof; the evidence file is absent, so there is no "
        "check to remove — only a read that cannot succeed",
    ),
    Audited(
        "test_malformed_and_oversized_evidence_fail_closed",
        "evidence past the byte cap is refused",
        (f"evidence exceeds {DRIVER.MAX_EVIDENCE_BYTES} bytes",),
    ),
    Audited(
        "test_malformed_and_oversized_evidence_fail_closed",
        "malformed JSON is refused; unparseable bytes yield no evidence to judge, so there is no "
        "check whose removal would admit them",
    ),
    *(
        Audited("test_every_positive_fact_is_asserted", f"{section}.{field} is asserted", messages)
        for section, field, _, messages in POSITIVE_FACTS
    ),
    Audited(
        "test_a_counter_mode_only_proof_does_not_prove_the_aead_derivation",
        "a run in which no role keyed with AEAD-GCM is not a proof of the AEAD derivation. "
        "Removing the requirement does not admit such a run: it leaves `validate_proof` with no "
        "witness to name, so the record it would have to publish cannot be built at all",
        ("no role negotiated an AEAD-GCM profile, so the AEAD key derivation is unproven",),
        IndexError,
    ),
    Audited(
        "test_the_negotiated_profile_must_be_a_name_the_registry_carries",
        "the SRTP profile is a name the RFC 5764 registry carries",
        ("the negotiated SRTP profile is not a name the registry carries",),
    ),
    Audited(
        "test_the_peer_and_its_exact_revision_are_recorded",
        "the peer browser and its revision are recorded",
        (
            "peer evidence must be an object",
            "peer browser name must be a non-empty string",
            "peer browser revision must be a non-empty string",
        ),
    ),
    Audited(
        "test_negatives_are_bound_to_validated_positive_and_layer",
        "a negative is bound to the positive it was recorded against",
        ("FingerprintMismatch negative is not bound to its validated positive evidence",),
    ),
    Audited(
        "test_negatives_are_bound_to_validated_positive_and_layer",
        "a negative failed at the layer it claims",
        ("fingerprint negative did not reach DTLS verification",),
    ),
    Audited(
        "test_the_two_ends_must_report_the_same_reversed_pair",
        "the browser's local candidate is sipx's nominated remote",
        ("browser local candidate differs from sipx nominated remote",),
    ),
)


class BrowserAudioProofTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="sipx-browser-proof-")
        self.directory = pathlib.Path(self.temporary.name)
        for role in DRIVER.ROLES:
            role_dir = self.directory / role
            role_dir.mkdir()
            b, s = browser(role), sipx(role)
            (role_dir / "browser.json").write_text(json.dumps(b), encoding="utf-8")
            (role_dir / "sipx.json").write_text(json.dumps(s), encoding="utf-8")
        compatibility = self.directory / "unused-rtcp-candidate"
        compatibility.mkdir()
        (compatibility / "browser.json").write_text(
            json.dumps(unused_rtcp_candidate_browser()), encoding="utf-8"
        )
        (compatibility / "sipx.json").write_text(
            json.dumps(sipx("browser-offerer")), encoding="utf-8"
        )
        negative_dir = self.directory / "negatives"
        negative_dir.mkdir()
        for name in ("FingerprintMismatch", "NoNominatedPair", "WeakerMedia"):
            role = "browser-offerer" if name == "FingerprintMismatch" else "browser-answerer"
            digest = DRIVER.proof_digest(browser(role), sipx(role))
            (negative_dir / f"{name}.json").write_text(
                json.dumps(negative(name, role, digest)), encoding="utf-8"
            )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def assert_refused(self, directory: pathlib.Path | None = None) -> None:
        with self.assertRaises(DRIVER.ProofError):
            DRIVER.validate_proof(directory or self.directory, PIN)

    def assert_test_needs(self, row: Audited) -> None:
        """Fail unless the audited test fails against a validator blind to the refusals it names.

        The test is re-run in its own fixture rather than inspected, so what is measured is the
        assertion as it actually executes. `assert_refused` raises `AssertionError` when the blind
        validator accepts the mutated evidence, which is the outcome proving the removed check was
        the only thing refusing it; `row.blind` records where that outcome is something else.
        """
        case = type(self)(row.test)
        case.setUp()
        try:
            with validator_blind_to(row.refusals), self.assertRaises(
                row.blind,
                msg=f"{row.test} still passes against a validator blind to {row.refusals!r}",
            ):
                getattr(case, row.test)()
        finally:
            case.tearDown()

    def rebind_negatives(self) -> None:
        """Re-derive each negative's binding digest from the evidence now on disk.

        Every negative carries the SHA-256 of the positive it was recorded against, so *any*
        edit to a role's evidence makes `validate_proof` refuse on that binding alone. A test
        that mutated a field and stopped there would be refused whatever the validator did with
        the field under test, and would pass against a validator that ignored it entirely.

        So every test that mutates a role's evidence calls this, and `PROOF_ASSERTIONS` is where
        that is checked rather than trusted.
        """
        for name in ("FingerprintMismatch", "NoNominatedPair", "WeakerMedia"):
            role = "browser-offerer" if name == "FingerprintMismatch" else "browser-answerer"
            digest = DRIVER.proof_digest(
                json.loads((self.directory / role / "browser.json").read_text(encoding="utf-8")),
                json.loads((self.directory / role / "sipx.json").read_text(encoding="utf-8")),
            )
            path = self.directory / "negatives" / f"{name}.json"
            value = json.loads(path.read_text(encoding="utf-8"))
            value["positive_sha256"] = digest
            path.write_text(json.dumps(value), encoding="utf-8")

    def test_complete_two_role_proof_is_accepted(self) -> None:
        result = DRIVER.validate_proof(self.directory, PIN)
        self.assertEqual(set(DRIVER.ROLES), set(result["roles"]))
        self.assertIn("unused_rtcp_candidate", result)

    def test_unused_candidate_compatibility_evidence_is_mandatory_and_exact(self) -> None:
        target = self.directory / "unused-rtcp-candidate/browser.json"
        original = unused_rtcp_candidate_browser()
        for components in (["1"], ["1", "2", "3"], ["2", "1"]):
            changed = copy.deepcopy(original)
            changed["sdp"]["local"]["candidate_components"] = components
            target.write_text(json.dumps(changed), encoding="utf-8")
            self.assert_refused()
        target.unlink()
        with self.assertRaises(OSError):
            DRIVER.validate_proof(self.directory, PIN)

    def test_one_role_cannot_be_called_complete(self) -> None:
        (self.directory / "browser-answerer/browser.json").unlink()
        with self.assertRaises((DRIVER.ProofError, OSError)):
            DRIVER.validate_proof(self.directory, PIN)

    def test_malformed_and_oversized_evidence_fail_closed(self) -> None:
        target = self.directory / "browser-offerer/browser.json"
        target.write_text("{", encoding="utf-8")
        self.assert_refused()
        # Valid evidence, padded past the cap — the file still parses to exactly what setUp wrote,
        # so the cap is the only thing standing between it and acceptance. A pad of whitespace
        # alone would be refused by the JSON parser whatever the cap did, and the assertion would
        # read as coverage of a limit nobody had shown was enforced.
        padded = json.dumps(browser("browser-offerer"))
        padded += " " * (DRIVER.MAX_EVIDENCE_BYTES + 1 - len(padded))
        target.write_text(padded, encoding="utf-8")
        self.assertGreater(target.stat().st_size, DRIVER.MAX_EVIDENCE_BYTES)
        self.assert_refused()

    def test_every_proof_assertion_is_non_vacuous(self) -> None:
        """Every assertion in the audit fails against a validator blind to the check it names."""
        for row in PROOF_ASSERTIONS:
            if not row.refusals:
                continue
            with self.subTest(test=row.test, claim=row.claim):
                self.assert_test_needs(row)

    def test_the_audit_covers_every_assertion_about_the_validator(self) -> None:
        """No test may reach `validate_proof` without a row in the audit.

        An assertion left out of `PROOF_ASSERTIONS` is one nobody has shown can fail, which is
        the state this whole file exists to make impossible to reach quietly.
        """
        audited = {row.test for row in PROOF_ASSERTIONS}
        reaching = {
            name
            for name in dir(type(self))
            if name.startswith("test_")
            and any(
                probe in inspect.getsource(getattr(type(self), name))
                for probe in VALIDATOR_PROBES
            )
        }
        self.assertEqual(reaching, audited)

    def test_every_positive_fact_is_asserted(self) -> None:
        original = browser("browser-offerer")
        target = self.directory / "browser-offerer/browser.json"
        for section, field, value, _ in POSITIVE_FACTS:
            with self.subTest(section=section, field=field):
                changed = copy.deepcopy(original)
                changed[section][field] = value
                target.write_text(json.dumps(changed), encoding="utf-8")
                self.rebind_negatives()
                self.assert_refused()

    def test_a_counter_mode_only_proof_does_not_prove_the_aead_derivation(self) -> None:
        """M-72: the run must refuse to call itself a proof when no role keyed with AEAD-GCM.

        RFC 7714 publishes no key-derivation vector, so where the 96-bit master salt sits in the
        PRF input block rests on a reading of the spec. Two sipx endpoints sharing a wrong
        reading interoperate perfectly with each other, and every round-trip test in the tree
        still passes. Only a foreign implementation deriving the same session keys can catch it,
        and only the AEAD profiles exercise that derivation — a counter-mode run proves the
        RFC 3711 derivation, which a published vector already pins.
        """
        for role in DRIVER.ROLES:
            target = self.directory / role / "browser.json"
            result = json.loads(target.read_text(encoding="utf-8"))
            result["security"]["srtp_profile"] = "AES_CM_128_HMAC_SHA1_80"
            target.write_text(json.dumps(result), encoding="utf-8")
        self.rebind_negatives()
        self.assert_refused()

    def test_the_negotiated_profile_must_be_a_name_the_registry_carries(self) -> None:
        """A profile field nothing checks records a string, not a negotiation.

        Presence alone is satisfied by any non-empty value, including a placeholder, so the
        evidence would survive the harness losing track of what was negotiated.

        The offerer role carries the mutation because it is the one no other assertion constrains
        to a set of profiles. An unregistered name in the answerer is refused by M-72's AEAD
        witness requirement as well, so this suite would keep passing with registry membership
        unchecked — the assertion would name a field it had stopped deciding anything about.
        """
        target = self.directory / "browser-offerer/browser.json"
        original = json.loads(target.read_text(encoding="utf-8"))
        for value in ("profile", "AEAD_AES_192_GCM", "aead_aes_256_gcm"):
            changed = copy.deepcopy(original)
            changed["security"]["srtp_profile"] = value
            target.write_text(json.dumps(changed), encoding="utf-8")
            self.rebind_negatives()
            self.assert_refused()

    def test_the_peer_and_its_exact_revision_are_recorded(self) -> None:
        """Evidence a stranger can audit has to say which build agreed with us.

        "A browser interoperated" is not a fact anyone can check twice; "this browser at this
        revision negotiated this profile" is.
        """
        target = self.directory / "browser-answerer/browser.json"
        original = json.loads(target.read_text(encoding="utf-8"))
        for mutation in ({}, {"browser_name": "chrome"}, {"browser_name": "chrome", "browser_version": ""}):
            changed = copy.deepcopy(original)
            changed["peer"] = mutation
            target.write_text(json.dumps(changed), encoding="utf-8")
            self.rebind_negatives()
            self.assert_refused()
        changed = copy.deepcopy(original)
        del changed["peer"]
        target.write_text(json.dumps(changed), encoding="utf-8")
        self.rebind_negatives()
        self.assert_refused()

    def test_negatives_are_bound_to_validated_positive_and_layer(self) -> None:
        target = self.directory / "negatives/FingerprintMismatch.json"
        original = json.loads(target.read_text(encoding="utf-8"))
        changed = copy.deepcopy(original)
        changed["positive_sha256"] = "0" * 64
        target.write_text(json.dumps(changed), encoding="utf-8")
        self.assert_refused()
        changed = copy.deepcopy(original)
        changed["browser"]["facts"]["dtls_state"] = "new"
        target.write_text(json.dumps(changed), encoding="utf-8")
        self.assert_refused()
        changed = copy.deepcopy(original)
        changed["browser"]["facts"]["dtls_state"] = "connecting"
        target.write_text(json.dumps(changed), encoding="utf-8")
        DRIVER.validate_proof(self.directory, PIN)

    def test_the_two_ends_must_report_the_same_reversed_pair(self) -> None:
        target = self.directory / "browser-answerer/sipx.json"
        changed = sipx("browser-answerer")
        changed["nominated_remote"] = "192.0.2.99:40000"
        target.write_text(json.dumps(changed), encoding="utf-8")
        self.rebind_negatives()
        self.assert_refused()

    def make_certificate(self) -> tuple[pathlib.Path, str]:
        key = self.directory / "key.pem"
        certificate = self.directory / "certificate.pem"
        subprocess.run(
            [
                "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-keyout", str(key), "-out", str(certificate), "-subj", "/CN=sipx.test",
                "-addext", "subjectAltName=DNS:sipx.test",
            ],
            check=True,
            capture_output=True,
        )
        return certificate, DRIVER.spki_pin(certificate.read_bytes(), "PEM")

    def test_pin_mismatch_starts_no_role(self) -> None:
        certificate, good_pin = self.make_certificate()
        marker = self.directory / "started"
        command = executable(self.directory / "marker.sh", f"touch {marker!s}\n")
        bad_pin = ("A" if good_pin[0] != "A" else "B") + good_pin[1:]
        completed = subprocess.run(
            [str(RUNNER), "--identity-probe", str(certificate), "sipx.test", bad_pin, str(command), str(self.directory / "out")],
            env={**os.environ, "SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT": "10"},
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertNotEqual(0, completed.returncode)
        self.assertFalse(marker.exists(), "identity failure admitted a role process")

    def test_interrupt_after_admission_kills_the_entire_process_group(self) -> None:
        pid_file = self.directory / "pids"
        probe = executable(
            self.directory / "fork.sh",
            'sleep 300 &\nchild=$!\nprintf "%s\\n%s\\n" "$$" "$child" >"$1"\nwait "$child"\n',
        )
        process = subprocess.Popen(
            [str(RUNNER), "--lifecycle-probe", str(probe), str(pid_file), str(self.directory / "out")],
            env={**os.environ, "SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT": "10", "SIPX_BROWSER_AUDIO_ROLE_TIMEOUT": "30"},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if pid_file.exists() and len(pid_file.read_text(encoding="utf-8").splitlines()) == 2:
                break
            self.assertIsNone(process.poll(), "the lifecycle probe exited before readiness")
            time.sleep(0.02)  # poll interval: the two-PID readiness file is the condition
        self.assertTrue(pid_file.exists(), "the lifecycle probe never reported readiness")
        pids = [int(value) for value in pid_file.read_text(encoding="utf-8").splitlines()]
        self.assertEqual(2, len(pids), "the leader and its child reported readiness")

        process.send_signal(signal.SIGTERM)
        _, stderr = process.communicate(timeout=15)
        self.assertEqual(124, process.returncode, stderr)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and any(pathlib.Path(f"/proc/{pid}").exists() for pid in pids):
            time.sleep(0.02)  # poll interval: /proc disappearance is the cleanup condition
        self.assertFalse([pid for pid in pids if pathlib.Path(f"/proc/{pid}").exists()])

    def test_complete_timeout_is_bounded_before_role_admission(self) -> None:
        probe = executable(self.directory / "timeout.sh", "sleep 300\n")
        completed = subprocess.run(
            [str(RUNNER), "--lifecycle-probe", str(probe), str(self.directory / "pids"), str(self.directory / "out")],
            env={**os.environ, "SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT": "1", "SIPX_BROWSER_AUDIO_ROLE_TIMEOUT": "30"},
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(124, completed.returncode, completed.stderr)
        self.assertIn("complete proof exceeded 1s", completed.stderr)

    def test_normal_exit_cleanup_kills_an_orphaned_group(self) -> None:
        pid_file = self.directory / "pids"
        probe = executable(
            self.directory / "orphan.sh",
            'sleep 300 &\nchild=$!\nprintf "%s\\n%s\\n" "$$" "$child" >"$1"\nexit 0\n',
        )
        completed = subprocess.run(
            [str(RUNNER), "--lifecycle-probe", str(probe), str(pid_file), str(self.directory / "out")],
            env={**os.environ, "SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT": "10"},
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(0, completed.returncode, completed.stderr)
        pids = [int(value) for value in pid_file.read_text(encoding="utf-8").splitlines()]
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and any(pathlib.Path(f"/proc/{pid}").exists() for pid in pids):
            time.sleep(0.02)  # poll interval: /proc disappearance is the cleanup condition
        self.assertFalse([pid for pid in pids if pathlib.Path(f"/proc/{pid}").exists()])

    def test_process_capture_is_hard_capped(self) -> None:
        flood = executable(
            self.directory / "flood.sh",
            'head -c 2097152 /dev/zero >"$(dirname "$0")/product-output"\n'
            "head -c 2097152 /dev/zero\n"
            "head -c 2097152 /dev/zero >&2\n",
        )
        output = self.directory / "capture"
        completed = subprocess.run(
            [str(RUNNER), "--capture-probe", str(flood), str(output)],
            env={**os.environ, "SIPX_BROWSER_AUDIO_TOTAL_TIMEOUT": "10"},
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(0, completed.returncode, completed.stderr)
        self.assertEqual(DRIVER.MAX_EVIDENCE_BYTES, (output / "capture.stdout").stat().st_size)
        self.assertEqual(DRIVER.MAX_EVIDENCE_BYTES, (output / "capture.stderr").stat().st_size)
        self.assertEqual(2 * DRIVER.MAX_EVIDENCE_BYTES, (self.directory / "product-output").stat().st_size)

    def test_small_readiness_output_is_visible_while_the_command_is_alive(self) -> None:
        marker = self.directory / "release"
        stdout = self.directory / "live.stdout"
        stderr = self.directory / "live.stderr"
        probe = executable(
            self.directory / "live.sh",
            'printf "ready\\n"\nwhile [[ ! -e $1 ]]; do sleep 0.02; done\n',
        )
        process = subprocess.Popen(
            [
                str(DRIVER_PATH),
                "bounded-run",
                "--stdout",
                str(stdout),
                "--stderr",
                str(stderr),
                "--limit",
                str(DRIVER.MAX_EVIDENCE_BYTES),
                "--",
                str(probe),
                str(marker),
            ]
        )
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if stdout.exists() and stdout.read_bytes() == b"ready\n":
                break
            time.sleep(0.02)  # poll interval: the live capture bytes are the readiness condition
        self.assertEqual(b"ready\n", stdout.read_bytes())
        marker.touch()
        self.assertEqual(0, process.wait(timeout=3))

    def test_application_negative_is_not_a_webdriver_protocol_error(self) -> None:
        observation = {"contract": DRIVER.CONTRACT, "error": "FingerprintMismatch"}
        self.assertEqual(observation, DRIVER.unwrap_webdriver_value({"value": observation}))
        with self.assertRaisesRegex(DRIVER.ProofError, "WebDriver: timeout"):
            DRIVER.unwrap_webdriver_value({"value": {"error": "timeout", "message": "late"}})


if __name__ == "__main__":
    unittest.main(verbosity=2)
