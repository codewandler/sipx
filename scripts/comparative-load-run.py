#!/usr/bin/env python3
"""Run one comparative-load execution and write its immutable evidence directory.

This is the X-99 orchestrator around the X-98 contract: it supervises one responder build and
the neutral driver through the fixed execution protocol — correctness preflight, the one-hundred
dialog qualification, the driver-headroom proof against the packaged minimal fixture, then the
six-rate ladder with five repetitions per rate and an early stop after two consecutive fully
failed rates — and assembles one validated result record per attempted repetition.

It is subject-neutral: everything endpoint-specific (identity, revision pin, build command,
responder argument vector) is read from an endpoint specification under ``docs/comparison/load``,
which is the one place a comparison subject may be named. Raw records are written before any
aggregate, and every process runs inside the X-98 process-group supervisor, whose EXIT/INT/TERM
cleanup owns every child until it is observably gone.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import importlib.util
import json
import os
import pathlib
import platform
import resource
import secrets
import signal
import subprocess
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
DRIVER = ROOT / "scripts" / "comparative-load-driver.py"


def _contract():
    spec = importlib.util.spec_from_file_location(
        "comparative_load", ROOT / "scripts" / "comparative-load.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


contract = _contract()

WARMUP_S = contract.WARMUP_MS // 1000
MEASUREMENT_S = contract.MEASUREMENT_MS // 1000
DRAIN_S = contract.MAX_DRAIN_MS // 1000
PREFLIGHT_DIALOGS = 20
QUALIFICATION_DIALOGS = 100
LOW_RATE = 1
PROVISIONAL_POLICY = "trying_100"
# The responder publishes a readiness `events` bound of max_active * 8, capped by the contract at
# 65,536, so the shared active limit stays at or below 8,192. It is far above any concurrency the
# ladder reaches at these ceilings, so it never becomes the binding constraint on a measured run.
ACTIVE_LIMIT = 8_192
SAMPLE_INTERVAL_MS = 1_000
INDEX_STRIDE = 3_000_000

#: Wall-clock caps for waiting on a child that owns its own bounded phases. These bound the
#: failure of the wait, not the phases themselves — the child's clocks own those.
DRIVER_WAIT_SLACK_S = 45


class RunError(Exception):
    """The execution cannot continue; everything already written stays as evidence."""


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_first(path: str) -> str:
    try:
        return pathlib.Path(path).read_text(encoding="utf-8").strip()
    except OSError:
        return "unavailable"


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def host_inventory() -> dict:
    return {
        "os": f"{platform.system()} {platform.release()}",
        "kernel": platform.version(),
        "architecture": platform.machine(),
        "logical_cpus": os.cpu_count() or 1,
        "memory_bytes": os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES"),
        "cpu_governor": read_first(
            "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"
        ),
        "clock": "CLOCK_MONOTONIC",
    }


def socket_limits() -> dict:
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    return {
        "rlimit_nofile_soft": soft,
        "rlimit_nofile_hard": hard,
        "rmem_max": int(read_first("/proc/sys/net/core/rmem_max") or 0),
        "wmem_max": int(read_first("/proc/sys/net/core/wmem_max") or 0),
        "rmem_default": int(read_first("/proc/sys/net/core/rmem_default") or 0),
        "wmem_default": int(read_first("/proc/sys/net/core/wmem_default") or 0),
    }


def tool_version(argv: list[str]) -> str:
    try:
        done = subprocess.run(argv, capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return "unavailable"
    return (done.stdout or done.stderr).strip().splitlines()[0]


def machine_inventory() -> dict:
    host = host_inventory()
    return {
        "os": host["os"],
        "architecture": host["architecture"],
        "logical_cpus": host["logical_cpus"],
        "memory_bytes": host["memory_bytes"],
        "clock": host["clock"],
    }


class ProcSampler:
    """Bounded /proc sampling of one process, joined before its evidence is read."""

    def __init__(self, pid: int) -> None:
        self.pid = pid
        self.stop = threading.Event()
        self.cpu_user_ms = 0
        self.cpu_system_ms = 0
        self.peak_rss_bytes = 0
        self.descriptor_high_water = 0
        self.task_thread_high_water = 0
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.tick_ms = 1000 // os.sysconf("SC_CLK_TCK")

    def _sample(self) -> None:
        try:
            stat = pathlib.Path(f"/proc/{self.pid}/stat").read_text().rsplit(") ", 1)[1].split()
            self.cpu_user_ms = int(stat[11]) * self.tick_ms
            self.cpu_system_ms = int(stat[12]) * self.tick_ms
            status = pathlib.Path(f"/proc/{self.pid}/status").read_text()
            for line in status.splitlines():
                if line.startswith("VmHWM:"):
                    self.peak_rss_bytes = int(line.split()[1]) * 1024
                elif line.startswith("Threads:"):
                    self.task_thread_high_water = max(
                        self.task_thread_high_water, int(line.split()[1])
                    )
            self.descriptor_high_water = max(
                self.descriptor_high_water, len(os.listdir(f"/proc/{self.pid}/fd"))
            )
        except (OSError, IndexError, ValueError):
            pass

    def _run(self) -> None:
        while not self.stop.wait(SAMPLE_INTERVAL_MS / 1000):
            self._sample()

    def __enter__(self) -> "ProcSampler":
        self._sample()
        self.thread.start()
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        self.stop.set()
        self.thread.join(timeout=5)
        self._sample()

    def resources(self, endpoint_active_high_water: int) -> dict:
        return {
            "sample_interval_ms": SAMPLE_INTERVAL_MS,
            "unsupported_resources": [],
            "cpu_user_ms": self.cpu_user_ms,
            "cpu_system_ms": self.cpu_system_ms,
            "peak_rss_bytes": self.peak_rss_bytes,
            "descriptor_high_water": self.descriptor_high_water,
            "task_thread_high_water": self.task_thread_high_water,
            "endpoint_active_high_water": endpoint_active_high_water,
        }


def last_json_with_schema(data: bytes, schema: str) -> dict | None:
    found = None
    for line in data.splitlines():
        try:
            candidate = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if isinstance(candidate, dict) and candidate.get("schema") == schema:
            found = candidate
    return found


def write_json(path: pathlib.Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=1, sort_keys=True) + "\n", encoding="utf-8")


def resolve_argv(template: list[str], values: dict[str, object]) -> list[str]:
    return [str(item).format(**values) for item in template]


class Execution:
    """One responder build measured through the whole fixed protocol."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.spec = json.loads(pathlib.Path(args.endpoint).read_text(encoding="utf-8"))
        self.driver_spec = json.loads(pathlib.Path(args.driver).read_text(encoding="utf-8"))
        self.ceiling = args.ceiling
        self.seed = args.seed
        self.run_id = args.run_id or secrets.token_hex(16)
        self.out = pathlib.Path(args.out or ROOT / "docs" / "comparison" / "load" / "runs" / self.run_id)
        self.phases = args.phases.split(",") if args.phases else [
            "preflight",
            "qualification",
            "headroom",
            "ladder",
        ]
        self.direction_index = args.direction_index
        self.resume = args.resume
        self.rates = contract.ladder_rates(self.ceiling)
        self.invocation = 0
        self.driver_artifact = pathlib.Path(self.driver_spec["artifact"])
        if not self.driver_artifact.is_absolute():
            self.driver_artifact = ROOT / self.driver_artifact
        self.responder_artifact = pathlib.Path(self.spec["artifact"])
        if not self.responder_artifact.is_absolute():
            self.responder_artifact = ROOT / self.responder_artifact
        self.supervisor = None
        self.machine = machine_inventory()
        self.command_line = " ".join(sys.argv)

    def retained_result(self, manifest: dict, rate_index: int, repetition: int) -> dict | None:
        """Return one already-written result on resume, after validating its exact filing."""
        if not self.resume:
            return None
        path = self.out / "results" / f"rate{rate_index}-rep{repetition}.json"
        if not path.is_file():
            return None
        result = json.loads(path.read_text(encoding="utf-8"))
        contract.validate_result(result, manifest)
        run = result["run"]
        if run["rate_index"] != rate_index or run["repetition"] != repetition:
            raise RunError(f"resume result {path} is filed under the wrong rate or repetition")
        return result

    # ------------------------------------------------------------------ manifest and env ----
    def responder_template(self) -> list[str]:
        return [
            str(self.responder_artifact) if item == "{artifact}" else item
            for item in self.spec["responder_argv"]
        ]

    def driver_template(self) -> list[str]:
        return [
            sys.executable,
            str(self.driver_artifact),
            "--role",
            "driver",
            "--target",
            "{target}",
            "--seed",
            "{seed}",
            "--run-id",
            self.run_id,
            "--max-active",
            str(ACTIVE_LIMIT),
            "--provisional",
            PROVISIONAL_POLICY,
        ]

    def fixture_template(self) -> list[str]:
        return [
            sys.executable,
            str(self.driver_artifact),
            "--role",
            "fixture",
            "--seed",
            "{seed}",
            "--run-id",
            self.run_id,
            "--max-active",
            str(ACTIVE_LIMIT),
            "--provisional",
            PROVISIONAL_POLICY,
        ]

    def manifest(self) -> dict:
        return {
            "schema": contract.MANIFEST_SCHEMA,
            "run_id": self.run_id,
            "seed": self.seed,
            "direction": {
                "index": self.direction_index,
                "driver": self.driver_spec["id"],
                "responder": self.spec["id"],
            },
            "builds": [
                {
                    "endpoint_id": self.driver_spec["id"],
                    "role": "driver",
                    "revision": self.driver_spec["revision"],
                    "artifact_sha256": sha256_file(self.driver_artifact),
                    "argv": self.driver_template(),
                    "cwd": str(ROOT),
                    "env_keys": ["PATH"],
                },
                {
                    "endpoint_id": self.spec["id"],
                    "role": "responder",
                    "revision": self.spec["revision"],
                    "artifact_sha256": sha256_file(self.responder_artifact),
                    "argv": self.responder_template(),
                    "cwd": str(ROOT),
                    "env_keys": ["PATH"],
                },
            ],
            "machine": self.machine,
            "ceiling": self.ceiling,
            "provisional_policy": PROVISIONAL_POLICY,
            "limits": {
                "active": ACTIVE_LIMIT,
                "events": contract.MAX_EVENTS,
                "event_bytes": contract.MAX_EVENT_BYTES,
                "stdout_bytes": contract.MAX_LOG_BYTES,
                "stderr_bytes": contract.MAX_LOG_BYTES,
            },
            "phases": {
                "readiness_ms": contract.READINESS_MS,
                "correctness_rate": 1,
                "correctness_dialogs": 20,
                "headroom_multiplier": 2,
                "warmup_ms": contract.WARMUP_MS,
                "measurement_ms": contract.MEASUREMENT_MS,
                "drain_ms": contract.MAX_DRAIN_MS,
            },
            "ladder": {
                "divisors": list(contract.LADDER_DIVISORS),
                "repetitions": contract.REPETITIONS,
                "stop_after_failed_rates": contract.STOP_AFTER_FAILED_RATES,
            },
        }

    def environment(self, manifest: dict) -> dict:
        builds = []
        for build, spec in (
            (manifest["builds"][0], self.driver_spec),
            (manifest["builds"][1], self.spec),
        ):
            builds.append(
                {
                    "endpoint_id": build["endpoint_id"],
                    "role": build["role"],
                    "revision": build["revision"],
                    "artifact": spec["artifact"],
                    "artifact_sha256": build["artifact_sha256"],
                    "build_command": spec["build_command"],
                    "toolchain": spec["toolchain"],
                    "features": spec.get("features", []),
                    "dependencies": spec.get("dependencies", []),
                }
            )
        return {
            "schema": "sipx.comparative-load.environment.v1",
            "captured_utc": utc_now(),
            "host": host_inventory(),
            "socket_limits": socket_limits(),
            "toolchains": [
                {"tool": "python3", "version": tool_version([sys.executable, "--version"])},
                {"tool": "rustc", "version": tool_version(["rustc", "--version"])},
                {"tool": "cargo", "version": tool_version(["cargo", "--version"])},
            ],
            "builds": builds,
            "commands": [self.command_line],
            "seed": self.seed,
            "contract_sha256": contract.contract_hash(),
        }

    # ---------------------------------------------------------------------- process pairs ----
    def start_responder(self, seed: int, duration_s: int):
        argv = resolve_argv(
            self.responder_template(),
            {
                "seed": seed,
                "duration_s": duration_s,
                "max_active": ACTIVE_LIMIT,
                "cleanup_s": 5,
            },
        )
        process = self.supervisor.start(argv, "responder")
        ready = process.wait_ready()
        return process, ready

    def start_fixture(self, seed: int):
        argv = resolve_argv(self.fixture_template(), {"seed": seed})
        process = self.supervisor.start(argv, "responder")
        ready = process.wait_ready()
        return process, ready

    def start_driver(self, target: str, seed: int, extra: list[str]):
        base = resolve_argv(self.driver_template(), {"target": target, "seed": seed})
        index_base = self.invocation * INDEX_STRIDE
        self.invocation += 1
        argv = base + ["--index-base", str(index_base)] + extra
        process = self.supervisor.start(argv, "driver")
        process.wait_ready()
        return process

    def wait_driver(self, process, budget_s: float) -> dict:
        try:
            process.process.wait(timeout=budget_s)
        except subprocess.TimeoutExpired as error:
            raise RunError("the driver outran its phase budget; supervisor cleanup owns it") from error
        escalation = process.close()
        if escalation != "none":
            raise RunError("driver cleanup escalated; its exit path is broken")
        summary = last_json_with_schema(bytes(process.stdout.data), "sipx.comparative-load.driver.v1")
        if summary is None:
            raise RunError("the driver emitted no summary record")
        return summary

    def stop_responder(self, process) -> tuple[dict | None, dict]:
        """SIGINT, bounded wait, close; returns (summary, cleanup evidence)."""
        began = time.monotonic()
        admission_stopped = True
        try:
            os.kill(process.process.pid, signal.SIGINT)
        except ProcessLookupError:
            pass
        try:
            process.process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            admission_stopped = False
        escalation = "none"
        group_exited = True
        pipes_closed = True
        try:
            escalation = process.close()
        except contract.ContractError:
            group_exited = False
            pipes_closed = False
            try:
                escalation = process.close()
            except contract.ContractError:
                escalation = "kill"
        leader_status = process.process.returncode
        if leader_status is None:
            leader_status = -9
        summary = last_json_with_schema(bytes(process.stdout.data), "sipx.load-responder.v1")
        if summary is None:
            summary = last_json_with_schema(
                bytes(process.stdout.data), "sipx.comparative-load.responder.v1"
            )
        cleanup = {
            "admission_stopped": admission_stopped,
            "process_group_exited": group_exited,
            "leader_status": int(leader_status),
            "descendant_pipe_eof": pipes_closed,
            "escalation": escalation,
            "elapsed_ms": int((time.monotonic() - began) * 1000),
        }
        return summary, cleanup

    # ---------------------------------------------------------------------------- phases ----
    def correctness_phase(self, phase: str, dialogs: int) -> dict:
        started = utc_now()
        began = time.monotonic()
        responder, ready = self.start_responder(self.seed, duration_s=dialogs + 60)
        driver = self.start_driver(
            ready["address"],
            self.seed,
            ["--rate", str(LOW_RATE), "--dialogs", str(dialogs), "--drain-s", str(DRAIN_S), "--max-active", "4"],
        )
        summary = self.wait_driver(driver, dialogs / LOW_RATE + DRAIN_S + DRIVER_WAIT_SLACK_S)
        responder_summary, cleanup = self.stop_responder(responder)
        counts = summary["counts"]
        post = (responder_summary or {}).get("post_drain", {})
        post_zero = (
            summary["post_drain"]["transactions"] == 0
            and responder_summary is not None
            and all(int(value) == 0 for value in post.values())
        )
        record = {
            "schema": "sipx.comparative-load.preflight.v1",
            "phase": phase,
            "rate_per_second": LOW_RATE,
            "dialogs": dialogs,
            "offered": counts["offered"],
            "completed": counts["completed"],
            "five_steps_observed": counts["offered"]
            == counts["established"]
            == counts["completed"]
            and summary["responses"]["final"].get("200", 0) == 2 * counts["completed"],
            "post_drain_zero": bool(post_zero),
            "passed": counts["offered"] == counts["completed"] == dialogs
            and bool(post_zero)
            and cleanup["leader_status"] == 0
            and cleanup["escalation"] == "none",
            "started_utc": started,
            "elapsed_ms": int((time.monotonic() - began) * 1000),
        }
        write_json(self.out / f"{phase}.json", record)
        return record

    def headroom_phase(self) -> dict:
        started = utc_now()
        began = time.monotonic()
        rate = 2 * self.ceiling
        fixture, ready = self.start_fixture(self.seed)
        driver = self.start_driver(
            ready["address"],
            self.seed,
            [
                "--rate",
                str(rate),
                "--warmup-s",
                str(WARMUP_S),
                "--measure-s",
                str(MEASUREMENT_S),
                "--drain-s",
                str(DRAIN_S),
            ],
        )
        summary = self.wait_driver(
            driver, WARMUP_S + MEASUREMENT_S + 2 * DRAIN_S + DRIVER_WAIT_SLACK_S
        )
        fixture_summary, cleanup = self.stop_responder(fixture)
        del fixture_summary
        counts = summary["counts"]
        offered = counts["offered"]
        completed = counts["completed"]
        cpu = summary["cpu_ms"]["user"] + summary["cpu_ms"]["system"]
        elapsed = max(summary["elapsed_ms"], 1)
        setup = summary["latency_ms"]["setup"] or {"p99": 0}
        record = {
            "schema": "sipx.comparative-load.headroom.v1",
            "fixture": "packaged minimal fixture (scripts/comparative-load-driver.py --role fixture)",
            "rate_per_second": rate,
            "offered": offered,
            "completed": completed,
            "completion_ratio": completed / offered if offered else 0.0,
            "setup_p99_ms": setup["p99"],
            "driver_cpu_fraction": cpu / elapsed,
            "passed": offered >= 1_000
            and completed * 1_000 >= offered * 999
            and setup["p99"] <= 250
            and cpu / elapsed < 0.8
            and summary["post_drain"]["transactions"] == 0
            and cleanup["escalation"] == "none",
            "started_utc": started,
            "elapsed_ms": int((time.monotonic() - began) * 1000),
        }
        write_json(self.out / "headroom.json", record)
        return record

    def repetition(self, manifest: dict, rate_index: int, repetition: int) -> dict:
        rate = self.rates[rate_index]
        derived = (
            self.seed
            ^ (self.direction_index << 56)
            ^ (rate_index << 32)
            ^ repetition
        )
        started = utc_now()
        began = time.monotonic()
        responder, ready = self.start_responder(derived, duration_s=150)
        with ProcSampler(responder.process.pid) as sampler:
            driver = self.start_driver(
                ready["address"],
                derived,
                [
                    "--rate",
                    str(rate),
                    "--warmup-s",
                    str(WARMUP_S),
                    "--measure-s",
                    str(MEASUREMENT_S),
                    "--drain-s",
                    str(DRAIN_S),
                ],
            )
            summary = self.wait_driver(
                driver, WARMUP_S + MEASUREMENT_S + 2 * DRAIN_S + DRIVER_WAIT_SLACK_S
            )
        responder_summary, cleanup = self.stop_responder(responder)
        elapsed_ms = int((time.monotonic() - began) * 1000)

        counts = dict(summary["counts"])
        errors = dict(summary["errors"])
        errors["evidence_overflow"] = 0
        errors["process_crash"] = 1 if cleanup["leader_status"] != 0 else 0
        responder_post = (responder_summary or {}).get("post_drain", {})
        responder_counts = (responder_summary or {}).get("counts", {})
        post_drain = {
            "active_dialogs": int(responder_post.get("active_dialogs", 0)),
            "transactions": int(responder_post.get("endpoint_transactions", 0))
            + int(summary["post_drain"]["transactions"]),
            "timers": int(summary["post_drain"]["timers"]),
            "endpoint_tasks": int(responder_post.get("owned_tasks", 0))
            + int(responder_post.get("dispatcher_routes", 0)),
            "retained_events": 0,
        }
        zero_state = all(value == 0 for value in post_drain.values())
        latency = {}
        for name in ("setup", "teardown"):
            metric = summary["latency_ms"][name]
            if metric is not None and metric["count"] > 0:
                latency[name] = metric
        drain_ms = min(summary["phases"]["drain_ms"], contract.MAX_DRAIN_MS)
        elapsed_floor = contract.WARMUP_MS + contract.MEASUREMENT_MS + drain_ms
        result = {
            "schema": contract.RESULT_SCHEMA,
            "status": "failed",
            "run": {
                "run_id": self.run_id,
                "seed": derived,
                "direction": manifest["direction"],
                "rate_index": rate_index,
                "rate_per_second": rate,
                "repetition": repetition,
                "started_utc": started,
                "elapsed_ms": max(elapsed_ms, elapsed_floor),
                "warmup_ms": contract.WARMUP_MS,
                "measurement_ms": contract.MEASUREMENT_MS,
                "drain_ms": drain_ms,
            },
            "build": {
                "endpoint_id": self.spec["id"],
                "role": "responder",
                "revision": self.spec["revision"],
                "artifact_sha256": manifest["builds"][1]["artifact_sha256"],
                "argv_sha256": contract.argv_hash(manifest["builds"][1]["argv"]),
            },
            "machine": self.machine,
            "profile": {
                "transport": "udp",
                "t1_ms": 500,
                "t2_ms": 4_000,
                "t4_ms": 5_000,
                "provisional_policy": PROVISIONAL_POLICY,
                "maximum_active": ACTIVE_LIMIT,
                "events": contract.MAX_EVENTS,
                "event_bytes": contract.MAX_EVENT_BYTES,
                "stdout_bytes": contract.MAX_LOG_BYTES,
                "stderr_bytes": contract.MAX_LOG_BYTES,
                "contract_sha256": contract.contract_hash(),
            },
            "counts": counts,
            "responses": summary["responses"],
            "errors": errors,
            "latency_ms": latency,
            "resources": sampler.resources(
                int(responder_counts.get("active_high_water", 0))
            ),
            "post_drain": post_drain,
            "cleanup": {
                "admission_stopped": True,
                "zero_state_observed": zero_state,
                "process_group_exited": cleanup["process_group_exited"],
                "leader_status": cleanup["leader_status"],
                "descendant_pipe_eof": cleanup["descendant_pipe_eof"],
                "escalation": cleanup["escalation"],
                "elapsed_ms": cleanup["elapsed_ms"],
            },
        }
        offered = counts["offered"]
        completed = counts["completed"]
        setup_p99 = latency.get("setup", {}).get("p99")
        passed = (
            offered >= 1_000
            and completed * 1_000 >= offered * 999
            and all(
                errors[name] == 0
                for name in (
                    "invalid_message",
                    "internal_error",
                    "cleanup_timeout",
                    "evidence_overflow",
                    "process_crash",
                )
            )
            and setup_p99 is not None
            and setup_p99 <= 250
            and zero_state
            and summary["phases"]["barrier_drained"]
            and cleanup["process_group_exited"]
            and cleanup["descendant_pipe_eof"]
            and cleanup["leader_status"] == 0
            and cleanup["escalation"] == "none"
        )
        result["status"] = "passed" if passed else "failed"
        contract.validate_result(result, manifest)
        write_json(self.out / "results" / f"rate{rate_index}-rep{repetition}.json", result)
        return result

    # ------------------------------------------------------------------------------ run ----
    def run(self) -> int:
        manifest = self.manifest()
        contract.validate_manifest(manifest)
        if self.resume:
            manifest_path = self.out / "manifest.json"
            environment_path = self.out / "environment.json"
            if not manifest_path.is_file() or not environment_path.is_file():
                raise RunError("resume requires the existing manifest and environment inventory")
            existing_manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            contract.validate_manifest(existing_manifest)
            if existing_manifest != manifest:
                raise RunError(
                    "resume manifest differs from the current artifacts or execution settings"
                )
            environment = json.loads(environment_path.read_text(encoding="utf-8"))
            commands = environment.get("commands")
            if not isinstance(commands, list):
                raise RunError("resume environment has no command inventory")
            if self.command_line not in commands:
                commands.append(self.command_line)
                write_json(environment_path, environment)
        else:
            write_json(self.out / "manifest.json", manifest)
            write_json(self.out / "environment.json", self.environment(manifest))

        with contract.ProcessSupervisor() as supervisor:
            self.supervisor = supervisor
            if "preflight" in self.phases:
                record = self.correctness_phase("preflight", PREFLIGHT_DIALOGS)
                if not record["passed"]:
                    print("preflight failed: not measured: correctness prerequisite failed")
                    return 3
            if "qualification" in self.phases:
                record = self.correctness_phase("qualification", QUALIFICATION_DIALOGS)
                if not record["passed"]:
                    print("qualification failed: not measured: correctness prerequisite failed")
                    return 3
            if "headroom" in self.phases:
                record = self.headroom_phase()
                if not record["passed"]:
                    print("headroom failed: the whole execution is invalid at this ceiling")
                    return 4
            omitted = []
            if "ladder" in self.phases:
                alive_history = []
                for rate_index in range(len(self.rates)):
                    if len(alive_history) >= 2 and not alive_history[-1] and not alive_history[-2]:
                        omitted.append(
                            {
                                "rate_index": rate_index,
                                "rate_per_second": self.rates[rate_index],
                                "reason": "two_consecutive_failed_rates",
                            }
                        )
                        continue
                    outcomes = []
                    for rep in range(contract.REPETITIONS):
                        result = self.retained_result(manifest, rate_index, rep)
                        retained = result is not None
                        if result is None:
                            result = self.repetition(manifest, rate_index, rep)
                        outcomes.append(result["status"] == "passed")
                        print(
                            f"rate {self.rates[rate_index]}/s repetition {rep}:"
                            f" {result['status']}" + (" (retained)" if retained else ""),
                            flush=True,
                        )
                    alive_history.append(any(outcomes))
                write_json(
                    self.out / "omissions.json",
                    {"schema": "sipx.comparative-load.omissions.v1", "omitted": omitted},
                )
        print(f"run {self.run_id} complete: evidence under {self.out}")
        return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True, help="endpoint spec JSON under docs/comparison/load")
    parser.add_argument("--driver", required=True, help="driver spec JSON under docs/comparison/load")
    parser.add_argument("--ceiling", type=int, required=True)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--run-id")
    parser.add_argument("--out")
    parser.add_argument("--direction-index", type=int, default=0, choices=(0, 1))
    parser.add_argument("--phases", help="comma list, default preflight,qualification,headroom,ladder")
    parser.add_argument(
        "--resume",
        action="store_true",
        help="continue an existing run only when its manifest exactly matches current artifacts",
    )
    args = parser.parse_args(argv)
    if args.resume and args.run_id is None:
        parser.error("--resume requires --run-id")
    try:
        return Execution(args).run()
    except (OSError, RunError, contract.ContractError) as error:
        print(f"comparative-load-run: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
