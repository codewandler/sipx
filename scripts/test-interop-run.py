#!/usr/bin/env python3
"""Tests for the interop harness's lifecycle (story `X-23`).

`X-23` was filed as "an interop call test times out one run in five". It is not a race inside a
test. Everything the harness reserves is machine-global: the peer runs on the host network under
one fixed container name, on fixed ports, and the call tests bind a fixed port of their own
because the peer's contact is written into its configuration before any test starts. A run's
start-up removes that container by name, and its cleanup removes every container carrying the
harness label — neither asks whether another run is using it.

So two runs on one machine are not two runs. The second one's start-up deletes the first one's
peer, and the first one's call tests then fail on their twenty-second timeout with nothing on the
far end. Both of them fail, together, which is what made the report look like something shared.

The measurement is in the story. What is pinned here is the property that fixes it: **two
concurrent runs of `run.sh` serialise**. The test drives the real script — copied verbatim, so it
cannot drift from the one people run — against a fixture peer and stub `docker` and `cargo`
commands, so it needs neither a container runtime nor a compiler and takes about three seconds.
"""

import os
import pathlib
import shutil
import subprocess
import tempfile
import textwrap
import threading
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
RUN_SH = ROOT / "tests" / "interop" / "run.sh"

# How long a stubbed `cargo test` pretends to take. It has to be long enough that a second run
# starting up lands inside the first one's test window, which is exactly where the reported
# failure landed.
TEST_SECONDS = "1.5"

DOCKER_STUB = """\
#!/usr/bin/env bash
# A stand-in for the container runtime. It records what the harness asked for, tagged with which
# run asked, and answers the three questions `run.sh` asks of a peer.
set -uo pipefail
say() { printf '%s %s\\n' "$RUN_TAG" "$1" >>"$EVENTS"; }

case "${1:-}" in
run)
    say run
    : >"$MARKER"
    echo "stub-container-id"
    ;;
logs)
    [[ -f "$MARKER" ]] && echo "the fixture peer is ready"
    ;;
exec)
    # `peer_check` greps these two.
    echo "Endpoint:  alice"
    echo "transport-tls"
    ;;
ps)
    [[ -f "$MARKER" ]] && echo "stub-container-id"
    ;;
rm)
    say rm
    rm -f "$MARKER"
    ;;
esac
exit 0
"""

CARGO_STUB = """\
#!/usr/bin/env bash
# A stand-in for cargo. `issue-certs` makes the directory the harness just removed; `test` takes
# long enough to be interrupted.
set -uo pipefail
say() { printf '%s %s\\n' "$RUN_TAG" "$1" >>"$EVENTS"; }

case "${1:-}" in
run)
    # `cargo run ... --example issue-certs -- <dir> <name>`: the directory is the argument after
    # the bare `--`.
    for ((i = 1; i <= $#; i++)); do
        if [[ "${!i}" == "--" ]]; then
            j=$((i + 1))
            mkdir -p "${!j}"
            : >"${!j}/ca.pem"
            break
        fi
    done
    ;;
test)
    say test-start
    sleep TEST_SECONDS_PLACEHOLDER
    say test-end
    [[ "${FAIL_CARGO_TEST:-0}" == "1" ]] && exit 1
    ;;
esac
exit 0
"""

PROFILE = """\
# A fixture peer. It claims both roles, so the harness runs both test phases and the window a
# concurrent run can land in is the real one.
PEER_TITLE="a fixture peer"
PEER_IMAGE="fixture:latest"
PEER_CONTAINER="sipx-fixture"
PEER_ROLES="server user-agent"
PEER_READY_MARKER="the fixture peer is ready"
PEER_ENV=()
PEER_DIVERGES_ON=()

peer_prepare() { :; }
peer_mounts() { printf '%s\\n' "$PEER_DIR/tls:/tls:ro"; }
peer_check() { [[ "${FAIL_PEER_CHECK:-0}" != "1" ]]; }
"""


class ConcurrentRuns(unittest.TestCase):
    """Two runs on one machine must take turns, because what they reserve is machine-global."""

    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="sipx-interop-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

        # The real script, copied rather than imported, beside a fixture peer. Copied so the test
        # exercises what people run; a reimplementation here would pass while `run.sh` was broken.
        harness = self.tmp / "interop"
        (harness / "peer").mkdir(parents=True)
        shutil.copy(RUN_SH, harness / "run.sh")
        (harness / "run.sh").chmod(0o755)
        (harness / "peer" / "profile.sh").write_text(PROFILE)
        self.run_sh = harness / "run.sh"

        binaries = self.tmp / "bin"
        binaries.mkdir()
        for name, body in (
            ("docker", DOCKER_STUB),
            ("cargo", CARGO_STUB.replace("TEST_SECONDS_PLACEHOLDER", TEST_SECONDS)),
        ):
            path = binaries / name
            path.write_text(body)
            path.chmod(0o755)

        self.events = self.tmp / "events"
        self.events.touch()
        self.env = dict(os.environ)
        self.env["PATH"] = f"{binaries}{os.pathsep}{self.env['PATH']}"
        self.env["EVENTS"] = str(self.events)
        self.env["MARKER"] = str(self.tmp / "marker")
        # A lock of this test's own, so the suite neither waits for nor blocks a real run.
        self.env["SIPX_INTEROP_LOCK"] = str(self.tmp / "lock")

    def start(self, tag: str) -> subprocess.Popen:
        env = dict(self.env, RUN_TAG=tag)
        return subprocess.Popen(
            [str(self.run_sh), "--peer", "peer"],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def test_a_second_run_waits_rather_than_deleting_the_first_ones_peer(self):
        first = self.start("A")
        # Started once the first is genuinely under way, which is the situation being pinned: not
        # two runs racing to start, but one arriving while another is mid-call.
        threading.Event().wait(0.4)
        second = self.start("B")

        for process in (first, second):
            process.communicate(timeout=120)

        tags = [line.split()[0] for line in self.events.read_text().splitlines() if line.strip()]
        self.assertTrue(tags, "the stubs recorded nothing; the harness never ran")
        self.assertIn("A", tags, "the first run recorded nothing")
        self.assertIn("B", tags, "the second run recorded nothing")

        # The whole property in one line: the tag changes hands exactly once. Any more and the
        # two runs were touching the peer at the same time, which is the defect.
        handovers = sum(1 for before, after in zip(tags, tags[1:]) if before != after)
        self.assertEqual(
            handovers,
            1,
            "the two runs interleaved rather than taking turns — a concurrent run removed the "
            f"other's peer mid-test. Events: {' '.join(tags)}",
        )

    def test_a_failed_role_test_makes_the_peer_and_runner_fail(self):
        env = dict(self.env, RUN_TAG="failed", FAIL_CARGO_TEST="1")
        completed = subprocess.run(
            [str(self.run_sh), "--peer", "peer"],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=120,
            check=False,
        )
        self.assertNotEqual(completed.returncode, 0, completed.stdout)
        self.assertIn("peer: FAILED", completed.stdout)

    def test_a_failed_peer_capability_check_stops_before_the_test_list(self):
        env = dict(self.env, RUN_TAG="failed-check", FAIL_PEER_CHECK="1")
        completed = subprocess.run(
            [str(self.run_sh), "--peer", "peer"],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=120,
            check=False,
        )
        self.assertNotEqual(completed.returncode, 0, completed.stdout)
        self.assertIn("peer: FAILED", completed.stdout)
        self.assertNotIn("test-start", self.events.read_text())


if __name__ == "__main__":
    unittest.main()
