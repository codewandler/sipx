#!/usr/bin/env python3
"""Adversarial self-test for the M-51 browser-audio proof harness."""

from __future__ import annotations

import copy
import importlib.util
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
            "srtp_profile": "profile",
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
        target.write_bytes(b" " * (DRIVER.MAX_EVIDENCE_BYTES + 1))
        self.assert_refused()

    def test_every_positive_fact_is_asserted(self) -> None:
        mutations = (
            ("codec", "mime_type", "audio/PCMU"),
            ("security", "dtls_state", "connecting"),
            ("security", "srtp_profile", ""),
            ("candidate_pair", "nominated", False),
            ("candidate_pair", "component", 2),
            ("media", "inbound_packets", 0),
            ("media", "received_audio_energy", 0),
        )
        original = browser("browser-offerer")
        target = self.directory / "browser-offerer/browser.json"
        for section, field, value in mutations:
            changed = copy.deepcopy(original)
            changed[section][field] = value
            target.write_text(json.dumps(changed), encoding="utf-8")
            self.assert_refused()
        changed = copy.deepcopy(original)
        changed["sip"]["order"].remove("bye-final")
        target.write_text(json.dumps(changed), encoding="utf-8")
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
