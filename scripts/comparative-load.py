#!/usr/bin/env python3
"""Validate and supervise the neutral comparative signalling-load contract.

The actual cross-endpoint measurement is an M14 operation.  This module is the M13 contract it
must pass through: closed manifest/result schemas plus process-group ownership whose output and
readiness waits are bounded.  It names no endpoint implementation.
"""

from __future__ import annotations

import argparse
import atexit
import hashlib
import ipaddress
import json
import os
import pathlib
import queue
import signal
import subprocess
import sys
import threading
from collections.abc import Callable, Mapping, Sequence
from typing import BinaryIO, NoReturn


MANIFEST_SCHEMA = "sipx.comparative-load.manifest.v1"
RESULT_SCHEMA = "sipx.comparative-load.result.v1"
READY_SCHEMA = "sipx.comparative-load.ready.v1"

READINESS_MS = 10_000
WARMUP_MS = 10_000
MEASUREMENT_MS = 60_000
MAX_DRAIN_MS = 40_000
REPETITIONS = 5
LADDER_DIVISORS = (32, 16, 8, 4, 2, 1)
STOP_AFTER_FAILED_RATES = 2
MAX_READY_BYTES = 4_096
MAX_LOG_BYTES = 16 * 1024 * 1024
MAX_EVENTS = 65_536
MAX_EVENT_BYTES = 64 * 1024 * 1024

HEX_32 = frozenset("0123456789abcdef")
ROLES = frozenset(("driver", "responder"))
STATUSES = frozenset(("passed", "failed", "environment_failed"))
TERMINAL_ERRORS = (
    "rejected",
    "transaction_timeout",
    "invalid_message",
    "transport_error",
    "admission_refused",
    "internal_error",
    "cleanup_timeout",
)
RUN_ERRORS = ("evidence_overflow", "process_crash")
RESOURCE_FIELDS = frozenset(
    (
        "cpu_user_ms",
        "cpu_system_ms",
        "peak_rss_bytes",
        "descriptor_high_water",
        "task_thread_high_water",
        "endpoint_active_high_water",
    )
)


class ContractError(ValueError):
    """The manifest, evidence or supervised lifecycle contradicted the v1 contract."""


def _fail(path: str, message: str) -> NoReturn:
    raise ContractError(f"{path}: {message}")


def _object(value: object, path: str) -> Mapping[str, object]:
    if not isinstance(value, dict):
        _fail(path, "must be an object")
    return value


def _closed(
    value: object,
    path: str,
    required: set[str] | frozenset[str],
    optional: set[str] | frozenset[str] = frozenset(),
) -> Mapping[str, object]:
    obj = _object(value, path)
    keys = set(obj)
    missing = required - keys
    unknown = keys - required - optional
    if missing:
        _fail(path, f"missing keys: {', '.join(sorted(missing))}")
    if unknown:
        _fail(path, f"unknown keys: {', '.join(sorted(unknown))}")
    return obj


def _text(value: object, path: str) -> str:
    if not isinstance(value, str) or not value:
        _fail(path, "must be a non-empty string")
    return value


def _positive_int(value: object, path: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        _fail(path, "must be a positive integer")
    return value


def _nonnegative_int(value: object, path: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        _fail(path, "must be a non-negative integer")
    return value


def _boolean(value: object, path: str) -> bool:
    if not isinstance(value, bool):
        _fail(path, "must be boolean")
    return value


def _sha256(value: object, path: str) -> str:
    text = _text(value, path)
    if len(text) != 64 or any(letter not in HEX_32 for letter in text):
        _fail(path, "must be 64 lowercase hexadecimal digits")
    return text


def _socket_address(value: object, path: str) -> str:
    text = _text(value, path)
    if text.startswith("["):
        close = text.find("]")
        if close < 0 or close + 1 >= len(text) or text[close + 1] != ":":
            _fail(path, "must be an IPv6 socket address in [address]:port form")
        host, port_text = text[1:close], text[close + 2 :]
    else:
        if text.count(":") != 1:
            _fail(path, "IPv6 socket addresses must use [address]:port form")
        try:
            host, port_text = text.rsplit(":", 1)
        except ValueError:
            _fail(path, "must be an IP socket address")
    try:
        ipaddress.ip_address(host)
        port = int(port_text, 10)
    except ValueError:
        _fail(path, "must be an IP socket address")
    if not 1 <= port <= 65_535 or str(port) != port_text:
        _fail(path, "port must be canonical decimal in 1..=65535")
    return text


def _run_id(value: object, path: str) -> str:
    text = _text(value, path)
    if len(text) != 32 or any(letter not in HEX_32 for letter in text):
        _fail(path, "must be 32 lowercase hexadecimal digits")
    return text


def _string_list(value: object, path: str, *, nonempty: bool = False) -> list[str]:
    if not isinstance(value, list) or (nonempty and not value):
        _fail(path, "must be a non-empty array" if nonempty else "must be an array")
    result: list[str] = []
    for index, item in enumerate(value):
        result.append(_text(item, f"{path}[{index}]"))
    return result


def contract_hash(spec: pathlib.Path | None = None) -> str:
    """Hash the normative contract bytes carried in every result."""

    if spec is None:
        spec = pathlib.Path(__file__).resolve().parents[1] / "docs/specs/comparative-load.md"
    return hashlib.sha256(spec.read_bytes()).hexdigest()


def argv_hash(argv: Sequence[str]) -> str:
    """Hash an argument vector without making whitespace or quoting part of its identity."""

    digest = hashlib.sha256()
    for argument in argv:
        encoded = argument.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def ladder_rates(ceiling: int) -> tuple[int, ...]:
    """Return the six preregistered rates, with no adaptive probing between them."""

    if isinstance(ceiling, bool) or not isinstance(ceiling, int) or ceiling <= 0:
        raise ContractError("ceiling must be a positive integer")
    return tuple(max(1, (ceiling + divisor - 1) // divisor) for divisor in LADDER_DIVISORS)


def omitted_after(rate_passes: Sequence[bool]) -> tuple[int, ...]:
    """Name higher rate indices omitted after two consecutive completed rate failures."""

    if len(rate_passes) > len(LADDER_DIVISORS) or any(type(value) is not bool for value in rate_passes):
        raise ContractError("rate outcomes must be at most six booleans")
    if len(rate_passes) >= 2 and not rate_passes[-1] and not rate_passes[-2]:
        return tuple(range(len(rate_passes), len(LADDER_DIVISORS)))
    return ()


def validate_manifest(value: object) -> Mapping[str, object]:
    """Validate one immutable execution manifest and return it unchanged."""

    manifest = _closed(
        value,
        "manifest",
        {
            "schema",
            "run_id",
            "seed",
            "direction",
            "builds",
            "machine",
            "ceiling",
            "limits",
            "phases",
            "ladder",
        },
    )
    if manifest["schema"] != MANIFEST_SCHEMA:
        _fail("manifest.schema", f"must be {MANIFEST_SCHEMA!r}")
    _run_id(manifest["run_id"], "manifest.run_id")
    seed = _nonnegative_int(manifest["seed"], "manifest.seed")
    if seed > (1 << 64) - 1:
        _fail("manifest.seed", "must fit an unsigned 64-bit integer")

    direction = _closed(
        manifest["direction"], "manifest.direction", {"index", "driver", "responder"}
    )
    direction_index = _nonnegative_int(direction["index"], "manifest.direction.index")
    if direction_index not in (0, 1):
        _fail("manifest.direction.index", "must be 0 or 1")
    driver = _text(direction["driver"], "manifest.direction.driver")
    responder = _text(direction["responder"], "manifest.direction.responder")
    if driver == responder:
        _fail("manifest.direction", "driver and responder must be distinct endpoint IDs")

    builds = manifest["builds"]
    if not isinstance(builds, list) or len(builds) != 2:
        _fail("manifest.builds", "must contain exactly the driver and responder builds")
    seen_roles: set[str] = set()
    seen_endpoints: set[str] = set()
    for index, item in enumerate(builds):
        path = f"manifest.builds[{index}]"
        build = _closed(
            item,
            path,
            {"endpoint_id", "role", "revision", "artifact_sha256", "argv", "cwd", "env_keys"},
        )
        endpoint = _text(build["endpoint_id"], f"{path}.endpoint_id")
        role = _text(build["role"], f"{path}.role")
        if role not in ROLES:
            _fail(f"{path}.role", "must be driver or responder")
        if role in seen_roles or endpoint in seen_endpoints:
            _fail(path, "roles and endpoint IDs must each be unique")
        seen_roles.add(role)
        seen_endpoints.add(endpoint)
        expected = direction[role]
        if endpoint != expected:
            _fail(f"{path}.endpoint_id", f"must equal direction.{role}")
        _text(build["revision"], f"{path}.revision")
        _sha256(build["artifact_sha256"], f"{path}.artifact_sha256")
        _string_list(build["argv"], f"{path}.argv", nonempty=True)
        _text(build["cwd"], f"{path}.cwd")
        env_keys = _string_list(build["env_keys"], f"{path}.env_keys")
        if len(env_keys) != len(set(env_keys)):
            _fail(f"{path}.env_keys", "must not contain duplicates")
    if seen_roles != ROLES or seen_endpoints != {driver, responder}:
        _fail("manifest.builds", "must match the direction exactly")

    machine = _closed(
        manifest["machine"],
        "manifest.machine",
        {"os", "architecture", "logical_cpus", "memory_bytes", "clock"},
    )
    _text(machine["os"], "manifest.machine.os")
    _text(machine["architecture"], "manifest.machine.architecture")
    _positive_int(machine["logical_cpus"], "manifest.machine.logical_cpus")
    _positive_int(machine["memory_bytes"], "manifest.machine.memory_bytes")
    _text(machine["clock"], "manifest.machine.clock")

    ceiling = _positive_int(manifest["ceiling"], "manifest.ceiling")
    rates = ladder_rates(ceiling)
    if len(set(rates)) != len(LADDER_DIVISORS):
        _fail("manifest.ceiling", "must produce six distinct rounded-up ladder rates")
    if rates[0] * (MEASUREMENT_MS // 1_000) < 1_000:
        _fail("manifest.ceiling", "lowest ladder rate must offer at least 1,000 measured dialogs")

    limits = _closed(
        manifest["limits"],
        "manifest.limits",
        {"active", "events", "event_bytes", "stdout_bytes", "stderr_bytes"},
    )
    _positive_int(limits["active"], "manifest.limits.active")
    if _positive_int(limits["events"], "manifest.limits.events") > MAX_EVENTS:
        _fail("manifest.limits.events", f"must not exceed {MAX_EVENTS}")
    if _positive_int(limits["event_bytes"], "manifest.limits.event_bytes") > MAX_EVENT_BYTES:
        _fail("manifest.limits.event_bytes", f"must not exceed {MAX_EVENT_BYTES}")
    for name in ("stdout_bytes", "stderr_bytes"):
        if _positive_int(limits[name], f"manifest.limits.{name}") > MAX_LOG_BYTES:
            _fail(f"manifest.limits.{name}", f"must not exceed {MAX_LOG_BYTES}")

    phases = _closed(
        manifest["phases"],
        "manifest.phases",
        {
            "readiness_ms",
            "correctness_rate",
            "correctness_dialogs",
            "headroom_multiplier",
            "warmup_ms",
            "measurement_ms",
            "drain_ms",
        },
    )
    exact_phases = {
        "readiness_ms": READINESS_MS,
        "correctness_rate": 1,
        "correctness_dialogs": 20,
        "headroom_multiplier": 2,
        "warmup_ms": WARMUP_MS,
        "measurement_ms": MEASUREMENT_MS,
    }
    for name, expected in exact_phases.items():
        actual = _positive_int(phases[name], f"manifest.phases.{name}")
        if actual != expected:
            _fail(f"manifest.phases.{name}", f"must be exactly {expected}")
    drain = _positive_int(phases["drain_ms"], "manifest.phases.drain_ms")
    if drain > MAX_DRAIN_MS:
        _fail("manifest.phases.drain_ms", f"must not exceed {MAX_DRAIN_MS}")

    ladder = _closed(
        manifest["ladder"],
        "manifest.ladder",
        {"divisors", "repetitions", "stop_after_failed_rates"},
    )
    if ladder["divisors"] != list(LADDER_DIVISORS):
        _fail("manifest.ladder.divisors", f"must be {list(LADDER_DIVISORS)}")
    if ladder["repetitions"] != REPETITIONS:
        _fail("manifest.ladder.repetitions", f"must be exactly {REPETITIONS}")
    if ladder["stop_after_failed_rates"] != STOP_AFTER_FAILED_RATES:
        _fail(
            "manifest.ladder.stop_after_failed_rates",
            f"must be exactly {STOP_AFTER_FAILED_RATES}",
        )
    return manifest


def _validate_percentiles(value: object, path: str) -> None:
    metrics = _closed(value, path, {"count", "p50", "p95", "p99", "max"})
    count = _positive_int(metrics["count"], f"{path}.count")
    values = [
        _nonnegative_int(metrics[name], f"{path}.{name}") for name in ("p50", "p95", "p99", "max")
    ]
    if values != sorted(values):
        _fail(path, "percentiles and maximum must be monotonically non-decreasing")
    if count == 0:
        _fail(path, "must be absent rather than carry zero samples")


def validate_result(value: object, manifest_value: object) -> Mapping[str, object]:
    """Validate one post-cleanup repetition against its immutable manifest."""

    manifest = validate_manifest(manifest_value)
    result = _closed(
        value,
        "result",
        {
            "schema",
            "status",
            "run",
            "build",
            "machine",
            "profile",
            "counts",
            "responses",
            "errors",
            "latency_ms",
            "resources",
            "post_drain",
            "cleanup",
        },
    )
    if result["schema"] != RESULT_SCHEMA:
        _fail("result.schema", f"must be {RESULT_SCHEMA!r}")
    status = _text(result["status"], "result.status")
    if status not in STATUSES:
        _fail("result.status", f"must be one of {sorted(STATUSES)}")

    run = _closed(
        result["run"],
        "result.run",
        {
            "run_id",
            "seed",
            "direction",
            "rate_index",
            "rate_per_second",
            "repetition",
            "started_utc",
            "elapsed_ms",
            "warmup_ms",
            "measurement_ms",
            "drain_ms",
        },
    )
    if run["run_id"] != manifest["run_id"]:
        _fail("result.run.run_id", "must match the manifest")
    run_seed = _nonnegative_int(run["seed"], "result.run.seed")
    if run_seed > (1 << 64) - 1:
        _fail("result.run.seed", "must fit an unsigned 64-bit integer")
    if run["direction"] != manifest["direction"]:
        _fail("result.run.direction", "must match the manifest")
    rate_index = _nonnegative_int(run["rate_index"], "result.run.rate_index")
    if rate_index >= len(LADDER_DIVISORS):
        _fail("result.run.rate_index", "must select one of the six ladder rates")
    ceiling = int(manifest["ceiling"])
    expected_rate = ladder_rates(ceiling)[rate_index]
    if run["rate_per_second"] != expected_rate:
        _fail("result.run.rate_per_second", f"must be the manifest ladder rate {expected_rate}")
    repetition = _nonnegative_int(run["repetition"], "result.run.repetition")
    if repetition >= REPETITIONS:
        _fail("result.run.repetition", f"must be in 0..{REPETITIONS - 1}")
    expected_seed = (
        int(manifest["seed"])
        ^ (int(manifest["direction"]["index"]) << 56)
        ^ (rate_index << 32)
        ^ repetition
    )
    if run_seed != expected_seed:
        _fail("result.run.seed", f"must be the derived repetition seed {expected_seed}")
    _text(run["started_utc"], "result.run.started_utc")
    _positive_int(run["elapsed_ms"], "result.run.elapsed_ms")
    if run["warmup_ms"] != WARMUP_MS or run["measurement_ms"] != MEASUREMENT_MS:
        _fail("result.run", "warmup and measurement durations must match the fixed profile")
    drain_ms = _nonnegative_int(run["drain_ms"], "result.run.drain_ms")
    if drain_ms > manifest["phases"]["drain_ms"]:
        _fail("result.run.drain_ms", "exceeds the manifest drain bound")

    build = _closed(
        result["build"],
        "result.build",
        {"endpoint_id", "role", "revision", "artifact_sha256", "argv_sha256"},
    )
    role = _text(build["role"], "result.build.role")
    if role not in ROLES or build["endpoint_id"] != manifest["direction"][role]:
        _fail("result.build", "endpoint and role must match the manifest direction")
    manifest_build = next(item for item in manifest["builds"] if item["role"] == role)
    if build["revision"] != manifest_build["revision"]:
        _fail("result.build.revision", "must match the immutable manifest")
    if build["artifact_sha256"] != manifest_build["artifact_sha256"]:
        _fail("result.build.artifact_sha256", "must match the immutable manifest")
    _sha256(build["artifact_sha256"], "result.build.artifact_sha256")
    _sha256(build["argv_sha256"], "result.build.argv_sha256")
    if build["argv_sha256"] != argv_hash(manifest_build["argv"]):
        _fail("result.build.argv_sha256", "must hash the manifest argument vector")

    if result["machine"] != manifest["machine"]:
        _fail("result.machine", "must exactly match the manifest inventory")
    profile = _closed(
        result["profile"],
        "result.profile",
        {
            "transport",
            "t1_ms",
            "t2_ms",
            "t4_ms",
            "maximum_active",
            "events",
            "event_bytes",
            "stdout_bytes",
            "stderr_bytes",
            "contract_sha256",
        },
    )
    expected_profile = {
        "transport": "udp",
        "t1_ms": 500,
        "t2_ms": 4_000,
        "t4_ms": 5_000,
        "maximum_active": manifest["limits"]["active"],
        "events": manifest["limits"]["events"],
        "event_bytes": manifest["limits"]["event_bytes"],
        "stdout_bytes": manifest["limits"]["stdout_bytes"],
        "stderr_bytes": manifest["limits"]["stderr_bytes"],
        "contract_sha256": contract_hash(),
    }
    if profile != expected_profile:
        _fail("result.profile", "must exactly describe the fixed contract and manifest limits")

    counts = _closed(
        result["counts"],
        "result.counts",
        {"offered", "established", "completed", "active_high_water", "request_retransmissions", "response_retransmissions"},
    )
    for name, item in counts.items():
        _nonnegative_int(item, f"result.counts.{name}")
    if counts["established"] > counts["offered"] or counts["completed"] > counts["established"]:
        _fail("result.counts", "must satisfy completed <= established <= offered")
    if counts["active_high_water"] > manifest["limits"]["active"]:
        _fail("result.counts.active_high_water", "exceeds the manifest bound")

    responses = _closed(result["responses"], "result.responses", {"provisional", "final"})
    for group in ("provisional", "final"):
        codes = _object(responses[group], f"result.responses.{group}")
        for code, amount in codes.items():
            if len(code) != 3 or not code.isdigit():
                _fail(f"result.responses.{group}", f"invalid SIP status key {code!r}")
            numeric_code = int(code)
            expected_range = range(100, 200) if group == "provisional" else range(200, 700)
            if numeric_code not in expected_range:
                _fail(f"result.responses.{group}", f"status {code} is in the wrong response class")
            _nonnegative_int(amount, f"result.responses.{group}.{code}")

    errors = _closed(result["errors"], "result.errors", set(TERMINAL_ERRORS + RUN_ERRORS))
    for name, item in errors.items():
        _nonnegative_int(item, f"result.errors.{name}")
    terminal = counts["completed"] + sum(errors[name] for name in TERMINAL_ERRORS)
    if terminal != counts["offered"]:
        _fail("result", "completed plus terminal error classes must equal offered")

    latency = _closed(result["latency_ms"], "result.latency_ms", set(), {"setup", "teardown"})
    if counts["established"] > 0 and "setup" not in latency:
        _fail("result.latency_ms.setup", "is required when a dialog was established")
    if counts["completed"] > 0 and "teardown" not in latency:
        _fail("result.latency_ms.teardown", "is required when a dialog completed")
    for name, metric in latency.items():
        _validate_percentiles(metric, f"result.latency_ms.{name}")
    if "setup" in latency and latency["setup"]["count"] != counts["established"]:
        _fail("result.latency_ms.setup.count", "must equal established dialogs")
    if "teardown" in latency and latency["teardown"]["count"] != counts["completed"]:
        _fail("result.latency_ms.teardown.count", "must equal completed dialogs")

    resources = _closed(
        result["resources"],
        "result.resources",
        {"sample_interval_ms", "unsupported_resources"},
        RESOURCE_FIELDS,
    )
    _positive_int(resources["sample_interval_ms"], "result.resources.sample_interval_ms")
    unsupported = _string_list(
        resources["unsupported_resources"], "result.resources.unsupported_resources"
    )
    if len(unsupported) != len(set(unsupported)) or any(name not in RESOURCE_FIELDS for name in unsupported):
        _fail("result.resources.unsupported_resources", "must be unique known resource fields")
    for name in RESOURCE_FIELDS:
        if name in resources and name in unsupported:
            _fail(f"result.resources.{name}", "cannot be both measured and unsupported")
        if name in resources:
            _nonnegative_int(resources[name], f"result.resources.{name}")
        elif name not in unsupported:
            _fail(f"result.resources.{name}", "must be measured or named unsupported")

    post_drain = _closed(
        result["post_drain"],
        "result.post_drain",
        {"active_dialogs", "transactions", "timers", "endpoint_tasks", "retained_events"},
    )
    for name, item in post_drain.items():
        _nonnegative_int(item, f"result.post_drain.{name}")

    cleanup = _closed(
        result["cleanup"],
        "result.cleanup",
        {
            "admission_stopped",
            "zero_state_observed",
            "process_group_exited",
            "leader_status",
            "descendant_pipe_eof",
            "escalation",
            "elapsed_ms",
        },
    )
    for name in ("admission_stopped", "zero_state_observed", "process_group_exited", "descendant_pipe_eof"):
        _boolean(cleanup[name], f"result.cleanup.{name}")
    if isinstance(cleanup["leader_status"], bool) or not isinstance(cleanup["leader_status"], int):
        _fail("result.cleanup.leader_status", "must be an integer process status")
    if cleanup["escalation"] not in ("none", "term", "kill"):
        _fail("result.cleanup.escalation", "must be none, term or kill")
    _nonnegative_int(cleanup["elapsed_ms"], "result.cleanup.elapsed_ms")

    if status == "passed":
        if counts["offered"] < 1_000 or counts["completed"] * 1_000 < counts["offered"] * 999:
            _fail("result.status", "passed requires at least 99.9% of at least 1,000 offers")
        for name in ("invalid_message", "internal_error", "cleanup_timeout") + RUN_ERRORS:
            if errors[name] != 0:
                _fail(f"result.errors.{name}", "must be zero for a passed result")
        setup = _object(latency.get("setup"), "result.latency_ms.setup")
        if _nonnegative_int(setup["p99"], "result.latency_ms.setup.p99") > 250:
            _fail("result.latency_ms.setup.p99", "must not exceed 250 for a passed result")
        if any(post_drain.values()):
            _fail("result.post_drain", "every count must be zero for a passed result")
        for name in ("admission_stopped", "zero_state_observed", "process_group_exited", "descendant_pipe_eof"):
            if cleanup[name] is not True:
                _fail(f"result.cleanup.{name}", "must be true for a passed result")
    return result


def validate_readiness(value: object, expected_role: str) -> Mapping[str, object]:
    """Validate the single bounded readiness record emitted by one process."""

    ready = _closed(value, "readiness", {"schema", "role", "pid", "address", "transport", "limits"})
    if ready["schema"] != READY_SCHEMA:
        _fail("readiness.schema", f"must be {READY_SCHEMA!r}")
    if ready["role"] != expected_role or expected_role not in ROLES:
        _fail("readiness.role", f"must be {expected_role!r}")
    _positive_int(ready["pid"], "readiness.pid")
    address = ready["address"]
    if expected_role == "responder":
        _socket_address(address, "readiness.address")
    elif address is not None:
        _socket_address(address, "readiness.address")
    if ready["transport"] != "udp":
        _fail("readiness.transport", "must be udp")
    limits = _closed(
        ready["limits"], "readiness.limits", {"active", "events", "stdout_bytes", "stderr_bytes"}
    )
    _positive_int(limits["active"], "readiness.limits.active")
    if _positive_int(limits["events"], "readiness.limits.events") > MAX_EVENTS:
        _fail("readiness.limits.events", f"must not exceed {MAX_EVENTS}")
    for name in ("stdout_bytes", "stderr_bytes"):
        if _positive_int(limits[name], f"readiness.limits.{name}") > MAX_LOG_BYTES:
            _fail(f"readiness.limits.{name}", f"must not exceed {MAX_LOG_BYTES}")
    return ready


class _BoundedReader:
    """Drain one inherited pipe without allowing evidence memory to grow without bound."""

    def __init__(self, stream: BinaryIO, limit: int, *, readiness: bool) -> None:
        self.stream = stream
        self.limit = limit
        self.readiness = readiness
        self.data = bytearray()
        self.overflow = threading.Event()
        self.eof = threading.Event()
        self.lines: queue.Queue[bytes] = queue.Queue(maxsize=1)
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self.thread.start()

    def _run(self) -> None:
        pending = bytearray()
        first_line_seen = False
        try:
            while True:
                # `BufferedReader.read(4096)` may wait for all 4096 bytes even though a complete
                # readiness line is already in the pipe. `os.read` returns the bytes currently
                # available; the readiness record, not a filled userspace buffer, is the event.
                chunk = os.read(self.stream.fileno(), 4096)
                if not chunk:
                    break
                room = self.limit - len(self.data)
                if room > 0:
                    self.data.extend(chunk[:room])
                if len(chunk) > room:
                    self.overflow.set()
                if self.readiness and not first_line_seen:
                    pending.extend(chunk)
                    if b"\n" in pending:
                        line, _, _rest = pending.partition(b"\n")
                        pending.clear()
                        first_line_seen = True
                        self.lines.put_nowait(bytes(line))
                    if len(pending) > MAX_READY_BYTES:
                        self.overflow.set()
        finally:
            if self.readiness and not first_line_seen and pending:
                try:
                    self.lines.put_nowait(bytes(pending))
                except queue.Full:
                    self.overflow.set()
            self.eof.set()


class SupervisedProcess:
    """One process group with bounded output, readiness and descendant cleanup."""

    def __init__(
        self,
        argv: Sequence[str],
        role: str,
        *,
        cwd: pathlib.Path | None = None,
        env: Mapping[str, str] | None = None,
        stdout_limit: int = MAX_LOG_BYTES,
        stderr_limit: int = MAX_LOG_BYTES,
    ) -> None:
        if os.name != "posix":
            raise ContractError("process-group supervision requires a POSIX host")
        if role not in ROLES:
            raise ContractError("role must be driver or responder")
        if not argv or any(not isinstance(item, str) or not item for item in argv):
            raise ContractError("argv must be a non-empty sequence of non-empty strings")
        if not 0 < stdout_limit <= MAX_LOG_BYTES or not 0 < stderr_limit <= MAX_LOG_BYTES:
            raise ContractError("output bounds must be positive and no larger than 16 MiB")
        self.role = role
        self.process = subprocess.Popen(
            list(argv),
            cwd=cwd,
            env=None if env is None else dict(env),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        if self.process.stdout is None or self.process.stderr is None:
            raise ContractError("supervised process pipes were not created")
        self.stdout = _BoundedReader(self.process.stdout, stdout_limit, readiness=True)
        self.stderr = _BoundedReader(self.process.stderr, stderr_limit, readiness=False)
        self.stdout.start()
        self.stderr.start()
        self._closed = False

    @property
    def pgid(self) -> int:
        return self.process.pid

    def wait_ready(self, timeout_ms: int = READINESS_MS) -> Mapping[str, object]:
        """Wait for the readiness fact, with the duration serving only as a failure bound."""

        if timeout_ms <= 0 or timeout_ms > READINESS_MS:
            raise ContractError(f"readiness bound must be in 1..={READINESS_MS} ms")
        try:
            line = self.stdout.lines.get(timeout=timeout_ms / 1000)
        except queue.Empty as error:
            self._close_after_failure()
            raise ContractError("readiness deadline expired before a record arrived") from error
        if self.stdout.overflow.is_set() or len(line) > MAX_READY_BYTES:
            self._close_after_failure()
            raise ContractError("readiness or stdout exceeded its configured bound")
        try:
            value = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            self._close_after_failure()
            raise ContractError(f"readiness is not one UTF-8 JSON object: {error}") from error
        try:
            ready = validate_readiness(value, self.role)
        except ContractError:
            self._close_after_failure()
            raise
        if ready["pid"] != self.process.pid:
            self._close_after_failure()
            raise ContractError("readiness pid does not identify the supervised group leader")
        return ready

    def _close_after_failure(self) -> None:
        """Preserve the primary readiness error while still owning every child to completion."""

        try:
            self.close(timeout_seconds=0.25)
        except ContractError:
            pass

    def _signal_group(self, signum: signal.Signals) -> None:
        try:
            os.killpg(self.pgid, signum)
        except ProcessLookupError:
            pass

    def close(
        self,
        graceful: Callable[[], None] | None = None,
        *,
        timeout_seconds: float = 5.0,
    ) -> str:
        """Stop admission, terminate the whole group if needed, and require inherited-pipe EOF."""

        if self._closed:
            return "none"
        if not 0 < timeout_seconds <= 5:
            raise ContractError("each cleanup wait bound must be in (0, 5] seconds")
        self._closed = True
        if graceful is not None:
            graceful()
        escalation = "none"
        try:
            self.process.wait(timeout=timeout_seconds)
        except subprocess.TimeoutExpired:
            escalation = "term"
            self._signal_group(signal.SIGTERM)
            try:
                self.process.wait(timeout=timeout_seconds)
            except subprocess.TimeoutExpired:
                escalation = "kill"
                self._signal_group(signal.SIGKILL)
                self.process.wait(timeout=timeout_seconds)

        # The leader may have exited while a child retained the group or an inherited pipe.
        self._signal_group(signal.SIGTERM)
        self.stdout.thread.join(timeout=timeout_seconds)
        self.stderr.thread.join(timeout=timeout_seconds)
        if self.stdout.thread.is_alive() or self.stderr.thread.is_alive():
            escalation = "kill"
            self._signal_group(signal.SIGKILL)
            self.stdout.thread.join(timeout=timeout_seconds)
            self.stderr.thread.join(timeout=timeout_seconds)
        if self.stdout.thread.is_alive() or self.stderr.thread.is_alive():
            raise ContractError("a descendant retained an inherited output pipe after group cleanup")
        self.process.stdout.close()
        self.process.stderr.close()
        if not self.stdout.eof.is_set() or not self.stderr.eof.is_set():
            raise ContractError("supervised output ended without observable EOF")
        if self.stdout.overflow.is_set() or self.stderr.overflow.is_set():
            raise ContractError("supervised output exceeded its configured bound")
        readiness_records = 0
        for line in bytes(self.stdout.data).splitlines():
            try:
                candidate = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            if isinstance(candidate, dict) and candidate.get("schema") == READY_SCHEMA:
                readiness_records += 1
        if readiness_records != 1:
            raise ContractError(
                f"supervised process emitted {readiness_records} readiness records, expected exactly one"
            )
        return escalation

    def __enter__(self) -> "SupervisedProcess":
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()


class ProcessSupervisor:
    """Own all process groups across normal exit, exceptions, SIGINT and SIGTERM."""

    def __init__(self, *, cleanup_wait_seconds: float = 5.0) -> None:
        if not 0 < cleanup_wait_seconds <= 5:
            raise ContractError("each cleanup wait bound must be in (0, 5] seconds")
        self.cleanup_wait_seconds = cleanup_wait_seconds
        self.processes: list[SupervisedProcess] = []
        self._previous: dict[signal.Signals, object] = {}
        self._entered = False
        self._closed = False

    def __enter__(self) -> "ProcessSupervisor":
        if self._entered:
            raise ContractError("a process supervisor cannot be entered twice")
        if threading.current_thread() is not threading.main_thread():
            raise ContractError("signal-safe process supervision must be entered on the main thread")
        self._entered = True
        for signum in (signal.SIGINT, signal.SIGTERM):
            self._previous[signum] = signal.getsignal(signum)
            signal.signal(signum, self._on_signal)
        atexit.register(self._on_exit)
        return self

    def start(
        self,
        argv: Sequence[str],
        role: str,
        **kwargs: object,
    ) -> SupervisedProcess:
        if not self._entered or self._closed:
            raise ContractError("enter the process supervisor before starting a process")
        process = SupervisedProcess(argv, role, **kwargs)
        self.processes.append(process)
        return process

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        problems: list[str] = []
        for process in reversed(self.processes):
            try:
                process.close(timeout_seconds=self.cleanup_wait_seconds)
            except ContractError as error:
                problems.append(str(error))
        if self._entered:
            for signum, previous in self._previous.items():
                signal.signal(signum, previous)
            atexit.unregister(self._on_exit)
        if problems:
            raise ContractError("; ".join(problems))

    def _on_exit(self) -> None:
        try:
            self.close()
        except ContractError as error:
            print(f"comparative-load cleanup: {error}", file=sys.stderr)

    def _on_signal(self, signum: int, frame: object) -> NoReturn:
        del frame
        try:
            self.close()
        finally:
            raise SystemExit(128 + signum)

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()


def _load_json(path: pathlib.Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"{path}: {error}") from error


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=pathlib.Path, required=True)
    parser.add_argument("--result", type=pathlib.Path)
    args = parser.parse_args(argv)
    try:
        manifest = _load_json(args.manifest)
        validate_manifest(manifest)
        if args.result is not None:
            validate_result(_load_json(args.result), manifest)
    except ContractError as error:
        print(f"comparative-load: {error}", file=sys.stderr)
        return 1
    print("comparative-load: manifest and result contract valid" if args.result else "comparative-load: manifest contract valid")
    return 0


if __name__ == "__main__":
    sys.exit(main())
