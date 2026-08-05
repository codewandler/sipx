#!/usr/bin/env python3
"""The neutral comparative-load driver, and its packaged minimal fixture responder.

This is the measuring instrument for `docs/specs/comparative-load.md`: a UDP-only caller that
offers the profile's exact `INVITE -> 2xx -> ACK -> BYE -> 2xx` dialogs at a fixed open-loop
rate, validates every response against the deterministic identifier contract, and reports one
machine-readable summary after an observed drain. It names no endpoint implementation.

Two roles share the file so the driver-headroom proof cannot drift from the driver it proves:

- ``--role driver`` offers dialogs to ``--target`` and measures them;
- ``--role fixture`` is the packaged minimal responder the headroom phase runs against.

Offered load is derived from the clock alone: a slowing target never lowers or raises what is
offered, which is the property the profile calls open-loop. Every phase, queue and wait in this
process is bounded, admission stops atomically on SIGINT/SIGTERM, and the process drains its own
transactions before reporting — the supervising runner owns the process group around it.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import heapq
import json
import os
import resource
import select
import signal
import socket
import sys
import time

T1_MS = 500
T2_MS = 4_000
TIMER_B_MS = 64 * T1_MS
MAX_UDP = 65_535
READY_SCHEMA = "sipx.comparative-load.ready.v1"
DRIVER_SCHEMA = "sipx.comparative-load.driver.v1"
FIXTURE_SCHEMA = "sipx.comparative-load.fixture.v1"

TERMINAL_CLASSES = (
    "completed",
    "rejected",
    "transaction_timeout",
    "invalid_message",
    "transport_error",
    "admission_refused",
    "internal_error",
    "cleanup_timeout",
)


def derive_identifiers(seed: int, run_id: str, index: int) -> dict[str, str]:
    """The §3.1 deterministic identifier set for one dialog index."""

    def digest(purpose: str, length: int) -> str:
        fields = (str(seed), run_id, str(index), purpose)
        material = b"\0".join(field.encode("utf-8") for field in fields)
        return hashlib.sha256(material).hexdigest()[:length]

    return {
        "call_id": f"cl-{run_id}-{index}@driver.invalid",
        "from_tag": f"f-{digest('from', 16)}",
        "to_tag": f"t-{digest('to', 16)}",
        "invite_branch": f"z9hG4bK-i-{digest('invite', 20)}",
        "ack_branch": f"z9hG4bK-a-{digest('ack', 20)}",
        "bye_branch": f"z9hG4bK-b-{digest('bye', 20)}",
    }


def parse_headers(datagram: bytes) -> tuple[str, dict[str, str]] | None:
    """First line plus a lowercase-name header map, or None for an unusable datagram."""
    try:
        text = datagram.decode("utf-8")
    except UnicodeDecodeError:
        return None
    head, _, _ = text.partition("\r\n\r\n")
    lines = head.split("\r\n")
    if not lines:
        return None
    headers: dict[str, str] = {}
    compact = {"v": "via", "f": "from", "t": "to", "i": "call-id", "m": "contact"}
    for line in lines[1:]:
        name, sep, value = line.partition(":")
        if not sep:
            continue
        key = name.strip().lower()
        key = compact.get(key, key)
        # The profile sends no folded or repeated headers; the first value wins and a
        # repeat would fail identifier validation anyway.
        headers.setdefault(key, value.strip())
    return lines[0], headers


def header_tag(value: str) -> str | None:
    for parameter in value.split(";")[1:]:
        name, sep, tag = parameter.partition("=")
        if sep and name.strip().lower() == "tag":
            return tag.strip()
    return None


def via_branch(value: str) -> str | None:
    for parameter in value.split(";")[1:]:
        name, sep, branch = parameter.partition("=")
        if sep and name.strip().lower() == "branch":
            return branch.strip()
    return None


def percentiles(samples: list[float]) -> dict[str, int] | None:
    """Nearest-rank percentiles in whole milliseconds, or None without samples."""
    if not samples:
        return None
    ordered = sorted(samples)
    count = len(ordered)

    def rank(hundredths: int) -> int:
        position = min(count, max(1, (count * hundredths + 99) // 100))
        return int(round(ordered[position - 1]))

    values = {
        "count": count,
        "p50": rank(50),
        "p95": rank(95),
        "p99": rank(99),
        "max": int(round(ordered[-1])),
    }
    # Rounding to whole milliseconds must never break the schema's monotonicity rule.
    ceiling = values["p50"]
    for name in ("p95", "p99", "max"):
        ceiling = max(ceiling, values[name])
        values[name] = ceiling
    return values


def emit_ready(role: str, address: str, limits: dict[str, int]) -> None:
    record = {
        "schema": READY_SCHEMA,
        "role": "driver" if role == "driver" else "responder",
        "pid": os.getpid(),
        "address": address,
        "transport": "udp",
        "limits": limits,
    }
    sys.stdout.write(json.dumps(record, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def cpu_ms() -> tuple[int, int]:
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return int(usage.ru_utime * 1000), int(usage.ru_stime * 1000)


class Interrupted(Exception):
    """Admission must stop atomically; the drain and the summary still run."""


class Driver:
    """One driver execution: paced admission, validation, and an observed drain."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.seed = args.seed
        self.run_id = args.run_id
        self.rate = args.rate
        self.max_active = args.max_active
        self.provisional = args.provisional
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        host, _, port = args.local.rpartition(":")
        self.sock.bind((host, int(port)))
        self.sock.setblocking(False)
        local_host, local_port = self.sock.getsockname()[:2]
        self.local = f"{local_host}:{local_port}"
        target_host, _, target_port = args.target.rpartition(":")
        self.target = (target_host, int(target_port))
        self.target_text = args.target

        self.timers: list[tuple[float, int, str, int]] = []
        self.timer_seq = 0
        self.dialogs: dict[int, dict] = {}
        self.by_call_id: dict[str, int] = {}
        self.done: dict[str, int] = {}
        self.active = 0
        self.stop_admission = False
        self.interrupted = False

        self.counting = False
        self.counts = {
            "offered": 0,
            "established": 0,
            "completed": 0,
            "active_high_water": 0,
            "request_retransmissions": 0,
            "response_retransmissions": 0,
        }
        self.errors = {name: 0 for name in TERMINAL_CLASSES if name != "completed"}
        self.responses: dict[str, dict[str, int]] = {"provisional": {}, "final": {}}
        self.setup_ms: list[float] = []
        self.teardown_ms: list[float] = []
        self.warmup = {"offered": 0, "completed": 0, "drained": True}

        signal.signal(signal.SIGINT, self._interrupt)
        signal.signal(signal.SIGTERM, self._interrupt)

    def _interrupt(self, signum: int, frame: object) -> None:
        del signum, frame
        self.stop_admission = True
        self.interrupted = True

    # ------------------------------------------------------------------ message building ----
    def build_messages(self, index: int) -> dict:
        ids = derive_identifiers(self.seed, self.run_id, index)
        target = self.target_text
        local = self.local
        invite = (
            f"INVITE sip:load@{target} SIP/2.0\r\n"
            f"Via: SIP/2.0/UDP {local};rport;branch={ids['invite_branch']}\r\n"
            "Max-Forwards: 70\r\n"
            f"From: <sip:driver@{local}>;tag={ids['from_tag']}\r\n"
            f"To: <sip:load@{target}>\r\n"
            f"Call-ID: {ids['call_id']}\r\n"
            "CSeq: 1 INVITE\r\n"
            f"Contact: <sip:driver@{local}>\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        ack = (
            f"ACK sip:load@{target} SIP/2.0\r\n"
            f"Via: SIP/2.0/UDP {local};rport;branch={ids['ack_branch']}\r\n"
            "Max-Forwards: 70\r\n"
            f"From: <sip:driver@{local}>;tag={ids['from_tag']}\r\n"
            f"To: <sip:load@{target}>;tag={ids['to_tag']}\r\n"
            f"Call-ID: {ids['call_id']}\r\n"
            "CSeq: 1 ACK\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        bye = (
            f"BYE sip:load@{target} SIP/2.0\r\n"
            f"Via: SIP/2.0/UDP {local};rport;branch={ids['bye_branch']}\r\n"
            "Max-Forwards: 70\r\n"
            f"From: <sip:driver@{local}>;tag={ids['from_tag']}\r\n"
            f"To: <sip:load@{target}>;tag={ids['to_tag']}\r\n"
            f"Call-ID: {ids['call_id']}\r\n"
            "CSeq: 2 BYE\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        return {"ids": ids, "invite": invite, "ack": ack, "bye": bye}

    # ------------------------------------------------------------------------ scheduling ----
    def arm(self, deadline: float, kind: str, index: int) -> None:
        self.timer_seq += 1
        heapq.heappush(self.timers, (deadline, self.timer_seq, kind, index))

    def send(self, payload: bytes, index: int) -> bool:
        try:
            self.sock.sendto(payload, self.target)
        except OSError:
            self.finish(index, "transport_error")
            return False
        return True

    def offer(self, index: int) -> None:
        if self.active >= self.max_active:
            # The bound exists so memory cannot grow without limit under a stalled target.
            # An offer the driver itself cannot create is its own state failure, not the
            # responder's admission refusal.
            self.record_offer()
            self.count_error("internal_error")
            return
        dialog = self.build_messages(index)
        now = time.monotonic()
        dialog.update(
            {
                "state": "calling",
                "counted": self.counting,
                "invite_at": now,
                "retransmit_ms": T1_MS,
                "provisional_seen": 0,
            }
        )
        self.dialogs[index] = dialog
        self.by_call_id[dialog["ids"]["call_id"]] = index
        self.active += 1
        if self.counting:
            self.counts["offered"] += 1
            self.counts["active_high_water"] = max(
                self.counts["active_high_water"], self.active
            )
        else:
            self.warmup["offered"] += 1
        if self.send(dialog["invite"], index):
            self.arm(now + T1_MS / 1000, "retransmit_invite", index)
            self.arm(now + TIMER_B_MS / 1000, "timeout", index)

    def record_offer(self) -> None:
        if self.counting:
            self.counts["offered"] += 1
        else:
            self.warmup["offered"] += 1

    def count_error(self, name: str) -> None:
        if self.counting:
            self.errors[name] += 1

    def finish(self, index: int, outcome: str) -> None:
        dialog = self.dialogs.pop(index, None)
        if dialog is None:
            return
        self.by_call_id.pop(dialog["ids"]["call_id"], None)
        self.done[dialog["ids"]["call_id"]] = index
        self.active -= 1
        if dialog["counted"]:
            if outcome == "completed":
                self.counts["completed"] += 1
            else:
                self.errors[outcome] += 1
        elif outcome == "completed":
            self.warmup["completed"] += 1

    def count_response(self, dialog: dict, group: str, code: str) -> None:
        if dialog["counted"]:
            bucket = self.responses[group]
            bucket[code] = bucket.get(code, 0) + 1

    # -------------------------------------------------------------------------- receiving ----
    def handle_datagram(self, datagram: bytes) -> None:
        parsed = parse_headers(datagram)
        if parsed is None:
            return
        first, headers = parsed
        if not first.startswith("SIP/2.0 "):
            return
        code_text = first[8:11]
        if not code_text.isdigit():
            return
        code = int(code_text)
        call_id = headers.get("call-id", "")
        index = self.by_call_id.get(call_id)
        if index is None:
            if call_id in self.done:
                # A retransmitted final for a finished dialog: re-ACK a 2xx INVITE final so
                # the responder can stop its own retransmission schedule.
                self.counts["response_retransmissions"] += 1 if self.counting else 0
                finished = self.build_messages(self.done[call_id])
                cseq = headers.get("cseq", "")
                if code == 200 and cseq.endswith("INVITE"):
                    try:
                        self.sock.sendto(finished["ack"], self.target)
                    except OSError:
                        pass
            return
        dialog = self.dialogs[index]
        ids = dialog["ids"]
        branch = via_branch(headers.get("via", ""))
        from_tag = header_tag(headers.get("from", ""))
        to_tag = header_tag(headers.get("to", ""))
        cseq = headers.get("cseq", "").split()
        state = dialog["state"]

        if state == "calling":
            if branch != ids["invite_branch"] or from_tag != ids["from_tag"] or cseq != ["1", "INVITE"]:
                self.finish(index, "invalid_message")
                return
            if code < 200:
                if code != 100 or self.provisional == "none":
                    self.finish(index, "invalid_message")
                    return
                if dialog["provisional_seen"]:
                    if dialog["counted"]:
                        self.counts["response_retransmissions"] += 1
                    return
                dialog["provisional_seen"] = 1
                self.count_response(dialog, "provisional", code_text)
                return
            if code == 200:
                contact = headers.get("contact", "")
                if (
                    to_tag != ids["to_tag"]
                    or f"sip:load@{self.target_text}" not in contact
                ):
                    self.finish(index, "invalid_message")
                    return
                if self.provisional == "trying_100" and not dialog["provisional_seen"]:
                    self.finish(index, "invalid_message")
                    return
                self.count_response(dialog, "final", code_text)
                if dialog["counted"]:
                    self.counts["established"] += 1
                    self.setup_ms.append((time.monotonic() - dialog["invite_at"]) * 1000)
                dialog["state"] = "bye_sent"
                dialog["retransmit_ms"] = T1_MS
                dialog["bye_at"] = time.monotonic()
                if self.send(dialog["ack"], index) and self.send(dialog["bye"], index):
                    now = time.monotonic()
                    self.arm(now + T1_MS / 1000, "retransmit_bye", index)
                    self.arm(now + TIMER_B_MS / 1000, "timeout", index)
                return
            # Valid non-2xx final: ACK it hop-by-hop with the INVITE branch, per RFC 3261.
            self.count_response(dialog, "final", code_text)
            failure_ack = (
                f"ACK sip:load@{self.target_text} SIP/2.0\r\n"
                f"Via: SIP/2.0/UDP {self.local};rport;branch={ids['invite_branch']}\r\n"
                "Max-Forwards: 70\r\n"
                f"From: <sip:driver@{self.local}>;tag={ids['from_tag']}\r\n"
                f"To: {headers.get('to', '')}\r\n"
                f"Call-ID: {ids['call_id']}\r\n"
                "CSeq: 1 ACK\r\n"
                "Content-Length: 0\r\n\r\n"
            ).encode()
            try:
                self.sock.sendto(failure_ack, self.target)
            except OSError:
                pass
            self.finish(index, "admission_refused" if code == 503 else "rejected")
            return

        if state == "bye_sent":
            if cseq == ["1", "INVITE"]:
                if dialog["counted"]:
                    self.counts["response_retransmissions"] += 1
                if code == 200:
                    self.send(dialog["ack"], index)
                return
            if (
                branch != ids["bye_branch"]
                or from_tag != ids["from_tag"]
                or to_tag != ids["to_tag"]
                or cseq != ["2", "BYE"]
            ):
                self.finish(index, "invalid_message")
                return
            if code < 200:
                self.finish(index, "invalid_message")
                return
            self.count_response(dialog, "final", code_text)
            if code == 200:
                if dialog["counted"]:
                    self.teardown_ms.append((time.monotonic() - dialog["bye_at"]) * 1000)
                self.finish(index, "completed")
            else:
                self.finish(index, "rejected")

    # ------------------------------------------------------------------------ event loop ----
    def pump(self, until: float) -> None:
        """Run timers and the socket until the monotonic deadline."""
        while True:
            now = time.monotonic()
            while self.timers and self.timers[0][0] <= now:
                _, _, kind, index = heapq.heappop(self.timers)
                dialog = self.dialogs.get(index)
                if dialog is None:
                    continue
                if kind == "timeout":
                    self.finish(index, "transaction_timeout")
                elif kind == "retransmit_invite" and dialog["state"] == "calling":
                    if self.send(dialog["invite"], index):
                        if dialog["counted"]:
                            self.counts["request_retransmissions"] += 1
                        dialog["retransmit_ms"] *= 2
                        self.arm(now + dialog["retransmit_ms"] / 1000, "retransmit_invite", index)
                elif kind == "retransmit_bye" and dialog["state"] == "bye_sent":
                    if self.send(dialog["bye"], index):
                        if dialog["counted"]:
                            self.counts["request_retransmissions"] += 1
                        dialog["retransmit_ms"] = min(dialog["retransmit_ms"] * 2, T2_MS)
                        self.arm(now + dialog["retransmit_ms"] / 1000, "retransmit_bye", index)
            if now >= until:
                return
            next_timer = self.timers[0][0] if self.timers else until
            wait = max(0.0, min(until, next_timer) - now)
            readable, _, _ = select.select([self.sock], [], [], min(wait, 0.05))
            if readable:
                for _ in range(512):
                    try:
                        datagram, _ = self.sock.recvfrom(MAX_UDP)
                    except BlockingIOError:
                        break
                    except OSError:
                        break
                    self.handle_datagram(datagram)

    def admit(self, count: int, first_index: int, rate: float) -> int:
        """Offer `count` dialogs at the fixed rate; returns how many were admitted."""
        start = time.monotonic()
        admitted = 0
        while admitted < count and not self.stop_admission:
            due = start + admitted / rate
            self.pump(due)
            if self.stop_admission:
                break
            self.offer(first_index + admitted)
            admitted += 1
        return admitted

    def drain(self, bound_seconds: float) -> bool:
        """Observe zero active dialogs, bounded; the bound only limits the failure."""
        deadline = time.monotonic() + bound_seconds
        while self.active > 0 and time.monotonic() < deadline:
            self.pump(min(deadline, time.monotonic() + 0.05))
        return self.active == 0

    def expire_leftovers(self) -> int:
        leftovers = list(self.dialogs)
        for index in leftovers:
            self.finish(index, "cleanup_timeout")
        return len(leftovers)

    def run(self) -> dict:
        args = self.args
        emit_ready(
            "driver",
            self.local,
            {
                "active": self.max_active,
                "events": 65_536,
                "stdout_bytes": 16 * 1024 * 1024,
                "stderr_bytes": 16 * 1024 * 1024,
            },
        )
        started_utc = datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        )
        cpu_before = cpu_ms()
        began = time.monotonic()
        barrier_drained = True
        warmup_ms = 0
        if args.warmup_s > 0:
            warmup_target = int(args.warmup_s * self.rate)
            warmup_started = time.monotonic()
            self.admit(warmup_target, args.index_base, self.rate)
            warmup_ms = int((time.monotonic() - warmup_started) * 1000)
            barrier_drained = self.drain(args.drain_s)
            if not barrier_drained:
                self.expire_leftovers()

        measurement_ms = 0
        drain_ms = 0
        if barrier_drained and not self.interrupted:
            self.counting = True
            measure_started = time.monotonic()
            if args.dialogs is not None:
                self.admit(args.dialogs, args.index_base + 1_000_000, self.rate)
            else:
                self.admit(
                    int(args.measure_s * self.rate),
                    args.index_base + 1_000_000,
                    self.rate,
                )
            measurement_ms = int((time.monotonic() - measure_started) * 1000)
            drain_started = time.monotonic()
            drained = self.drain(args.drain_s)
            drain_ms = int((time.monotonic() - drain_started) * 1000)
            if not drained:
                self.expire_leftovers()

        cpu_after = cpu_ms()
        elapsed_ms = int((time.monotonic() - began) * 1000)
        summary = {
            "schema": DRIVER_SCHEMA,
            "role": "driver",
            "target": self.target_text,
            "rate_per_second": self.rate,
            "interrupted": self.interrupted,
            "started_utc": started_utc,
            "elapsed_ms": elapsed_ms,
            "phases": {
                "warmup_ms": warmup_ms,
                "barrier_drained": barrier_drained,
                "measurement_ms": measurement_ms,
                "drain_ms": drain_ms,
            },
            "warmup": self.warmup,
            "counts": self.counts,
            "responses": self.responses,
            "errors": self.errors,
            "latency_ms": {
                "setup": percentiles(self.setup_ms),
                "teardown": percentiles(self.teardown_ms),
            },
            "post_drain": {"transactions": self.active, "timers": len(self.timers) if self.active else 0},
            "cpu_ms": {
                "user": cpu_after[0] - cpu_before[0],
                "system": cpu_after[1] - cpu_before[1],
            },
        }
        return summary


class Fixture:
    """The packaged minimal responder the driver-headroom phase runs against."""

    def __init__(self, args: argparse.Namespace) -> None:
        self.seed = args.seed
        self.run_id = args.run_id
        self.max_active = args.max_active
        self.provisional = args.provisional
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        host, _, port = args.local.rpartition(":")
        self.sock.bind((host, int(port)))
        self.sock.setblocking(False)
        local_host, local_port = self.sock.getsockname()[:2]
        self.local = f"{local_host}:{local_port}"
        self.active: dict[str, float] = {}
        self.stopping = False
        self.counts = {"invites": 0, "completed": 0, "refused": 0, "active_high_water": 0}
        signal.signal(signal.SIGINT, self._interrupt)
        signal.signal(signal.SIGTERM, self._interrupt)

    def _interrupt(self, signum: int, frame: object) -> None:
        del signum, frame
        self.stopping = True

    def to_tag(self, call_id: str) -> str:
        prefix = f"cl-{self.run_id}-"
        suffix = "@driver.invalid"
        index = "0"
        if call_id.startswith(prefix) and call_id.endswith(suffix):
            index = call_id[len(prefix) : -len(suffix)]
        material = b"\0".join(
            (str(self.seed).encode(), self.run_id.encode(), index.encode(), b"to")
        )
        return "t-" + hashlib.sha256(material).hexdigest()[:16]

    def respond(self, headers: dict[str, str], address, extra_to_tag: str | None) -> None:
        to_value = headers.get("to", "")
        if extra_to_tag is not None and "tag=" not in to_value:
            to_value = f"{to_value};tag={extra_to_tag}"
        response = (
            "SIP/2.0 200 OK\r\n"
            f"Via: {headers.get('via', '')}\r\n"
            f"From: {headers.get('from', '')}\r\n"
            f"To: {to_value}\r\n"
            f"Call-ID: {headers.get('call-id', '')}\r\n"
            f"CSeq: {headers.get('cseq', '')}\r\n"
            f"Contact: <sip:load@{self.local}>\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        try:
            self.sock.sendto(response, address)
        except OSError:
            pass

    def trying(self, headers: dict[str, str], address) -> None:
        response = (
            "SIP/2.0 100 Trying\r\n"
            f"Via: {headers.get('via', '')}\r\n"
            f"From: {headers.get('from', '')}\r\n"
            f"To: {headers.get('to', '')}\r\n"
            f"Call-ID: {headers.get('call-id', '')}\r\n"
            f"CSeq: {headers.get('cseq', '')}\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        try:
            self.sock.sendto(response, address)
        except OSError:
            pass

    def refuse(self, headers: dict[str, str], address) -> None:
        response = (
            "SIP/2.0 503 Service Unavailable\r\n"
            f"Via: {headers.get('via', '')}\r\n"
            f"From: {headers.get('from', '')}\r\n"
            f"To: {headers.get('to', '')};tag={self.to_tag(headers.get('call-id', ''))}\r\n"
            f"Call-ID: {headers.get('call-id', '')}\r\n"
            f"CSeq: {headers.get('cseq', '')}\r\n"
            "Content-Length: 0\r\n\r\n"
        ).encode()
        try:
            self.sock.sendto(response, address)
        except OSError:
            pass

    def run(self) -> dict:
        emit_ready(
            "fixture",
            self.local,
            {
                "active": self.max_active,
                "events": 65_536,
                "stdout_bytes": 16 * 1024 * 1024,
                "stderr_bytes": 16 * 1024 * 1024,
            },
        )
        started_utc = datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        )
        began = time.monotonic()
        while not self.stopping:
            readable, _, _ = select.select([self.sock], [], [], 0.05)
            if not readable:
                continue
            for _ in range(512):
                try:
                    datagram, address = self.sock.recvfrom(MAX_UDP)
                except (BlockingIOError, OSError):
                    break
                parsed = parse_headers(datagram)
                if parsed is None:
                    continue
                first, headers = parsed
                call_id = headers.get("call-id", "")
                if first.startswith("INVITE "):
                    self.counts["invites"] += 1
                    if call_id in self.active:
                        self.respond(headers, address, self.to_tag(call_id))
                        continue
                    if len(self.active) >= self.max_active:
                        self.counts["refused"] += 1
                        self.refuse(headers, address)
                        continue
                    self.active[call_id] = time.monotonic()
                    self.counts["active_high_water"] = max(
                        self.counts["active_high_water"], len(self.active)
                    )
                    if self.provisional == "trying_100":
                        self.trying(headers, address)
                    self.respond(headers, address, self.to_tag(call_id))
                elif first.startswith("BYE "):
                    if call_id in self.active:
                        del self.active[call_id]
                        self.counts["completed"] += 1
                        self.respond(headers, address, None)
                    else:
                        self.respond(headers, address, None)
                # ACK needs no reply; anything else is outside the fixture's vocabulary.
        return {
            "schema": FIXTURE_SCHEMA,
            "role": "fixture",
            "started_utc": started_utc,
            "elapsed_ms": int((time.monotonic() - began) * 1000),
            "counts": self.counts,
            "post_drain": {"active_dialogs": len(self.active)},
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--role", choices=("driver", "fixture"), required=True)
    parser.add_argument("--target", default="")
    parser.add_argument("--local", default="127.0.0.1:0")
    parser.add_argument("--rate", type=float, default=1.0)
    parser.add_argument("--dialogs", type=int)
    parser.add_argument("--warmup-s", type=float, default=0.0)
    parser.add_argument("--measure-s", type=float, default=0.0)
    parser.add_argument("--drain-s", type=float, default=40.0)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--run-id", default="0" * 32)
    parser.add_argument("--index-base", type=int, default=0)
    parser.add_argument("--max-active", type=int, default=32_768)
    parser.add_argument("--provisional", choices=("none", "trying_100"), default="none")
    args = parser.parse_args(argv)

    if args.role == "driver":
        if not args.target:
            parser.error("--target is required for the driver role")
        if args.dialogs is None and args.measure_s <= 0:
            parser.error("the driver needs a finite bound: --dialogs or --measure-s")
        if args.rate <= 0 or args.max_active <= 0 or args.drain_s <= 0:
            parser.error("rate, active and drain bounds must be positive")
        summary = Driver(args).run()
    else:
        summary = Fixture(args).run()
    sys.stdout.write(json.dumps(summary, separators=(",", ":")) + "\n")
    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
