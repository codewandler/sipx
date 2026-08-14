#!/usr/bin/env python3
"""Run one reproduction step with a deadline and process-group cleanup."""

from __future__ import annotations

import os
import signal
import subprocess
import sys


def stop_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    process.wait()


def main() -> int:
    if len(sys.argv) < 3:
        raise SystemExit("usage: bounded.py SECONDS COMMAND [ARG ...]")
    timeout = float(sys.argv[1])
    process = subprocess.Popen(sys.argv[2:], start_new_session=True)
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        stop_group(process)
        raise SystemExit(f"command exceeded {timeout:g} seconds")
    finally:
        if process.poll() is None:
            stop_group(process)


if __name__ == "__main__":
    raise SystemExit(main())
