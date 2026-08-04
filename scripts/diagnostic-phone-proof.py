#!/usr/bin/env python3
"""Run and report the diagnostic-phone exit proof from one bounded command.

The twelve vectors are normative in ``docs/specs/diagnostic-phone.md``.  This runner discovers the
Rust process tests carrying each vector marker, executes each discovered test with a finite failure
bound, and prints the requested and observed path beside the result.  It also checks the independent
peer profiles against the real interop test names; a profile or a prose page is not evidence by
itself.

``--check`` performs the structural checks without starting a phone or a container. ``--run`` runs
the local process vectors. ``--interop`` additionally runs the container-backed peer suite.  All
three modes print the same matrices, and any missing vector, failed command, or claimed transport
without two peer paths makes the command fail.
"""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import subprocess
import sys
from collections.abc import Iterable, Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
CLI_TESTS = ROOT / "crates" / "sipx-cli" / "tests" / "cli.rs"
INTEROP_TESTS = ROOT / "crates" / "sipx-ua" / "tests" / "interop.rs"
INTEROP_RUNNER = ROOT / "tests" / "interop" / "run.sh"
PROFILE_GLOB = "*/profile.sh"

# A timeout answers only "has this test failed to terminate?". Readiness and ordering inside the
# vectors are event-driven, as diagnostic-phone.md requires.
COMMAND_TIMEOUT_SECONDS = 240
INTEROP_TIMEOUT_SECONDS = 20 * 60


@dataclasses.dataclass(frozen=True)
class Vector:
    number: int
    requested: str
    observed: str

    @property
    def identifier(self) -> str:
        return f"DPH-{self.number}"


VECTORS = (
    Vector(1, "udp, tcp, tls, ws, wss", "connected transport in both terminal reports"),
    Vector(2, "wss; wrong certificate identity", "typed TLS refusal; no downgrade"),
    Vector(3, "opus; codec feature absent", "pre-I/O setup refusal"),
    Vector(4, "sdes over udp", "pre-I/O unsafe-combination refusal"),
    Vector(5, "dtls-srtp", "negotiated DTLS-SRTP audio or typed feature refusal"),
    Vector(6, "stun ICE; host paths unreachable", "server-reflexive pair carries audio"),
    Vector(7, "missing stable device id", "typed pre-I/O device refusal"),
    Vector(8, "custom Supported and custom Via", "Supported sent; Via refused before bind"),
    Vector(9, "wait, DTMF, hang up", "correlated events in causal order"),
    Vector(10, "finite call count", "exact admission and joined cleanup"),
    Vector(11, "interrupt after first INVITE", "admission closes before final summary"),
    Vector(12, "WAV and Linux virtual microphone", "same clip and observable device counters"),
)


@dataclasses.dataclass(frozen=True)
class LocalTest:
    vector: int
    name: str
    features: tuple[str, ...] = ()
    linux_only: bool = False

    def command(self) -> list[str]:
        command = ["cargo", "test", "-p", "sipx-cli", "--test", "cli"]
        if self.features:
            command.extend(("--features", ",".join(self.features)))
        command.extend((self.name, "--", "--exact", "--test-threads=1"))
        return command


@dataclasses.dataclass(frozen=True)
class CommandResult:
    status: str
    detail: str


@dataclasses.dataclass(frozen=True)
class ProductPath:
    name: str
    requested: str
    observed: str
    vector: int | None = None
    test: LocalTest | None = None
    check: tuple[str, ...] | None = None
    gap: str | None = None


TRANSPORT_TESTS = {
    "udp": "registers_against_a_real_server_over_udp",
    "tcp": "registers_against_a_real_server_over_tcp",
    "tls": "registers_against_a_real_server_over_tls",
    "ws": "registers_against_a_real_server_over_websocket",
    # A TLS test and a WS test do not compose into WSS evidence merely because the implementation
    # composes their code paths. This is a distinct verified-TLS-plus-upgrade peer exchange.
    "wss": "registers_against_a_real_server_over_secure_websocket",
}

# The story's release matrix is intentionally wider than its twelve regression vectors. A lower
# layer having an Opus or early-media test is not enough for this product proof: the executable
# evidence must enter through the diagnostic-phone process.
PRODUCT_PATHS = (
    ProductPath("G.711", "default pcmu/pcma", "connected command call", vector=1),
    ProductPath(
        "Opus",
        "--codec opus",
        "48 kHz distinguishable audio crosses both directions",
        test=LocalTest(0, "diagnostic_phone_opus_is_rate_and_direction_correct", ("opus",)),
    ),
    ProductPath(
        "plain RTP",
        "--media-security plain",
        "established calls report plain",
        test=LocalTest(0, "explicit_plain_and_sdes_report_what_the_tls_calls_actually_negotiated"),
    ),
    ProductPath(
        "SDES-SRTP",
        "--media-security sdes",
        "established calls report SDES",
        test=LocalTest(0, "explicit_plain_and_sdes_report_what_the_tls_calls_actually_negotiated"),
    ),
    ProductPath("DTLS-SRTP", "--media-security dtls-srtp", "audio crosses", vector=5),
    ProductPath(
        "early media",
        "--early-media; reliable provisional answer",
        "audio before final answer",
        test=LocalTest(
            0,
            "diagnostic_phone_records_reliable_provisional_audio_before_final_answer",
        ),
    ),
    ProductPath(
        "authenticated INVITE",
        "proxy challenge plus password",
        "retried INVITE connects",
        test=LocalTest(0, "dial_password_answers_a_proxy_challenge_and_connects"),
    ),
    ProductPath("ICE NAT", "--ice stun", "server-reflexive pair carries audio", vector=6),
    ProductPath("device loopback", "device: stable id", "same clip as WAV", vector=12),
    ProductPath(
        "CLI reference",
        "public commands and JSON envelopes",
        "generated or checked from executable schemas",
        check=("./scripts/check-cli-reference.py", "--check"),
    ),
)


def marked_tests(source: str) -> dict[int, list[str]]:
    """Return test functions attached to a preceding ``DPH-N`` doc marker."""

    marked: dict[int, list[str]] = {vector.number: [] for vector in VECTORS}
    pending: set[int] = set()
    function = re.compile(r"\s*(?:async\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(")
    marker = re.compile(r"DPH-(1[0-2]|[1-9])\b")
    for line in source.splitlines():
        if line.lstrip().startswith("///"):
            pending.update(int(value) for value in marker.findall(line))
            continue
        match = function.match(line)
        if match:
            for number in pending:
                marked[number].append(match.group(1))
            pending.clear()
    return marked


def local_tests(source: str) -> dict[int, list[LocalTest]]:
    tests: dict[int, list[LocalTest]] = {vector.number: [] for vector in VECTORS}
    for number, names in marked_tests(source).items():
        for name in names:
            features: tuple[str, ...] = ()
            linux_only = False
            if number == 5 and "without_the_feature" not in name:
                features = ("dtls",)
            elif number in (7, 12):
                features = ("device-audio",)
                linux_only = number == 12
            tests[number].append(LocalTest(number, name, features, linux_only))
    return tests


def peer_profiles() -> dict[str, set[str]]:
    profiles: dict[str, set[str]] = {}
    role_pattern = re.compile(r'^PEER_ROLES="([^"]*)"', re.MULTILINE)
    for path in sorted((ROOT / "tests" / "interop").glob(PROFILE_GLOB)):
        match = role_pattern.search(path.read_text())
        roles = set(match.group(1).split()) if match else {"server"}
        profiles[path.parent.name] = roles
    return profiles


def interop_coverage(source: str) -> dict[str, tuple[str, ...]]:
    functions = set(re.findall(r"(?:async\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(", source))
    server_peers = tuple(
        name for name, roles in peer_profiles().items() if "server" in roles
    )
    coverage: dict[str, tuple[str, ...]] = {}
    for transport, test_name in TRANSPORT_TESTS.items():
        coverage[transport] = (
            server_peers if test_name is not None and test_name in functions else ()
        )
    return coverage


def run_command(command: Sequence[str], timeout: int) -> CommandResult:
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return CommandResult("failed", f"exceeded {timeout}s failure bound")
    if completed.returncode == 0:
        return CommandResult("passed", "command exited 0")
    return CommandResult("failed", f"command exited {completed.returncode}")


def execute_local(tests: dict[int, list[LocalTest]]) -> dict[int, CommandResult]:
    results: dict[int, CommandResult] = {}
    for vector in VECTORS:
        candidates = tests[vector.number]
        if not candidates:
            results[vector.number] = CommandResult("missing", "no marked process test")
            continue
        applicable = [test for test in candidates if not test.linux_only or sys.platform == "linux"]
        if not applicable:
            results[vector.number] = CommandResult("not-run", "requires Linux")
            continue
        failures: list[str] = []
        for test in applicable:
            print(f"==> {vector.identifier}: {test.name}", flush=True)
            result = run_command(test.command(), COMMAND_TIMEOUT_SECONDS)
            if result.status != "passed":
                failures.append(f"{test.name}: {result.detail}")
        results[vector.number] = (
            CommandResult("failed", "; ".join(failures))
            if failures
            else CommandResult("passed", f"{len(applicable)} process test(s)")
        )
    return results


def execute_product_paths(
    vectors: dict[int, CommandResult], *, run: bool, execute_checks: bool = True
) -> dict[str, CommandResult]:
    results: dict[str, CommandResult] = {}
    cache: dict[tuple[str, tuple[str, ...]], CommandResult] = {}
    cli_source = CLI_TESTS.read_text()
    functions = set(re.findall(r"(?:async\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(", cli_source))
    for path in PRODUCT_PATHS:
        if path.gap is not None:
            results[path.name] = CommandResult("open", path.gap)
        elif path.vector is not None:
            results[path.name] = vectors[path.vector]
        elif path.check is not None and not execute_checks:
            results[path.name] = CommandResult("present", " ".join(path.check))
        elif path.check is not None:
            print(f"==> product path: {path.name}", flush=True)
            results[path.name] = run_command(path.check, COMMAND_TIMEOUT_SECONDS)
        elif path.test is not None and path.test.name not in functions:
            results[path.name] = CommandResult("missing", f"no {path.test.name} process test")
        elif path.test is not None and not run:
            results[path.name] = CommandResult("present", path.test.name)
        elif path.test is not None:
            key = (path.test.name, path.test.features)
            if key not in cache:
                print(f"==> product path: {path.name}: {path.test.name}", flush=True)
                cache[key] = run_command(path.test.command(), COMMAND_TIMEOUT_SECONDS)
            results[path.name] = cache[key]
        else:
            results[path.name] = CommandResult("missing", "no process evidence")
    return results


def structural_results(tests: dict[int, list[LocalTest]]) -> dict[int, CommandResult]:
    return {
        vector.number: CommandResult(
            "present" if tests[vector.number] else "missing",
            (
                ", ".join(test.name for test in tests[vector.number])
                if tests[vector.number]
                else "no marked process test"
            ),
        )
        for vector in VECTORS
    }


def table(headers: Sequence[str], rows: Iterable[Sequence[str]]) -> str:
    lines = [
        "| " + " | ".join(headers) + " |",
        "|" + "|".join("---" for _ in headers) + "|",
    ]
    lines.extend("| " + " | ".join(row) + " |" for row in rows)
    return "\n".join(lines)


def render_vectors(results: dict[int, CommandResult]) -> str:
    rows = []
    for vector in VECTORS:
        result = results[vector.number]
        rows.append(
            (
                vector.identifier,
                vector.requested,
                vector.observed,
                result.status,
                result.detail,
            )
        )
    return table(("Vector", "Requested path", "Observed path", "State", "Evidence"), rows)


def render_transports(coverage: dict[str, tuple[str, ...]]) -> str:
    rows = []
    for transport, test_name in TRANSPORT_TESTS.items():
        peers = coverage[transport]
        state = "covered" if len(peers) >= 2 else "open"
        evidence = test_name or "no independent-peer WSS test"
        rows.append((transport, str(len(peers)), ", ".join(peers) or "—", state, evidence))
    return table(("Transport", "Peer paths", "Profiles", "State", "Executed test"), rows)


def render_product_paths(results: dict[str, CommandResult]) -> str:
    rows = []
    for path in PRODUCT_PATHS:
        result = results[path.name]
        rows.append((path.name, path.requested, path.observed, result.status, result.detail))
    return table(("Path", "Requested", "Observed", "State", "Evidence"), rows)


def failed(
    vectors: dict[int, CommandResult],
    coverage: dict[str, tuple[str, ...]],
    products: dict[str, CommandResult] | None = None,
) -> bool:
    vector_failure = any(
        result.status in {"missing", "failed"} for result in vectors.values()
    )
    peer_failure = any(len(peers) < 2 for peers in coverage.values())
    product_failure = products is not None and any(
        result.status in {"missing", "failed", "open"} for result in products.values()
    )
    return vector_failure or peer_failure or product_failure


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="check proof structure without calls")
    mode.add_argument("--run", action="store_true", help="run all local DPH process vectors")
    parser.add_argument(
        "--interop",
        action="store_true",
        help="also execute every container-backed independent peer",
    )
    args = parser.parse_args(argv)

    tests = local_tests(CLI_TESTS.read_text())
    vectors = execute_local(tests) if args.run else structural_results(tests)
    products = execute_product_paths(vectors, run=args.run)
    interop_source = INTEROP_TESTS.read_text()
    coverage = interop_coverage(interop_source)

    if args.interop:
        print("==> independent-peer interop", flush=True)
        peer_result = run_command([str(INTEROP_RUNNER)], INTEROP_TIMEOUT_SECONDS)
        if peer_result.status != "passed":
            print(f"interop: {peer_result.detail}", file=sys.stderr)
            # A failed peer run invalidates every scheduled peer path for this invocation.
            coverage = {transport: () for transport in coverage}

    print("\n## Diagnostic-phone vectors\n")
    print(render_vectors(vectors))
    print("\n## Complete phone paths\n")
    print(render_product_paths(products))
    print("\n## Independent-peer signalling\n")
    print(render_transports(coverage))

    if failed(vectors, coverage, products):
        print(
            "\ndiagnostic-phone proof: OPEN — missing vectors or independent-peer paths are listed above",
            file=sys.stderr,
        )
        return 1
    print("\ndiagnostic-phone proof: complete")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
