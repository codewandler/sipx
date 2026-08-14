#!/usr/bin/env python3
"""Build M-72's broken-KDF witness from a disposable source copy.

The published SRTP crate must not contain a selectable broken-crypto path. This helper instead
copies the checkout, verifies one exact source fragment, changes only that copy, builds the native
browser proof endpoint, records the source and binary hashes, and removes the copy.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess


ROOT = pathlib.Path(__file__).resolve().parents[2]
KDF_SOURCE = pathlib.Path("crates/sipx-rtp/src/srtp/mod.rs")
SIDECAR = "browser_audio_proof.kdf-perturbation.json"
SENTINEL = ".sipx-perturbed-kdf-copy"
DEFAULT_BUILD_TIMEOUT = 300.0
GROUP_STOP_TIMEOUT = 3.0

ORIGINAL = """    let slot = iv
        .get_mut(..master_salt.len())
        .ok_or(SrtpError::KeyLength {
"""

PERTURBED = """    // M-72 proof mutation: right-align the salt in RFC 3711's 14-octet x value.
    // This changes a 12-octet AEAD salt from 0..12 to 2..14 and leaves counter mode unchanged.
    let salt_offset = MASTER_SALT_LEN.saturating_sub(master_salt.len());
    let salt_end = salt_offset.saturating_add(master_salt.len());
    let slot = iv
        .get_mut(salt_offset..salt_end)
        .ok_or(SrtpError::KeyLength {
"""


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def checked_source(source: pathlib.Path) -> tuple[pathlib.Path, bytes, bytes]:
    path = source / KDF_SOURCE
    original = path.read_bytes()
    text = original.decode("utf-8")
    if text.count(ORIGINAL) != 1:
        raise SystemExit(
            f"perturbed-KDF build: expected exactly one audited KDF fragment in {path}"
        )
    if PERTURBED in text:
        raise SystemExit("perturbed-KDF build: source checkout is already mutated")
    changed = text.replace(ORIGINAL, PERTURBED, 1).encode("utf-8")
    if original == changed:
        raise SystemExit("perturbed-KDF build: mutation changed no bytes")
    return path, original, changed


def ignored(_directory: str, names: list[str]) -> set[str]:
    return set(names).intersection({".git", ".claude", "node_modules", "target", "__pycache__"})


def refuse_unsafe_paths(source: pathlib.Path, work: pathlib.Path, target: pathlib.Path) -> None:
    work_inside_ignored_source = False
    if source in work.parents:
        relative = work.relative_to(source)
        work_inside_ignored_source = bool(relative.parts) and relative.parts[0] in {
            ".claude",
            "target",
        }
    if (
        work == pathlib.Path("/")
        or work == source
        or work in source.parents
        or (source in work.parents and not work_inside_ignored_source)
    ):
        raise SystemExit("perturbed-KDF build: disposable work path overlaps the source checkout")
    if target == pathlib.Path("/") or target == work or target in work.parents or work in target.parents:
        raise SystemExit("perturbed-KDF build: target path overlaps the disposable source copy")
    if work.exists():
        raise SystemExit(f"perturbed-KDF build: disposable work path already exists: {work}")


def interrupted(_signum: int, _frame: object) -> None:
    raise BuildInterrupted("perturbed-KDF build interrupted")


class BuildInterrupted(RuntimeError):
    """The owning helper received INT or TERM while its build was running."""


class BuildTimedOut(RuntimeError):
    """The disposable proof build exceeded its finite bound."""


def stop_process_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        process.wait()
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=GROUP_STOP_TIMEOUT)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def run_bounded_build(
    command: list[str], cwd: pathlib.Path, environment: dict[str, str], timeout: float
) -> None:
    if timeout <= 0:
        raise ValueError("perturbed-KDF build timeout must be positive")
    process = subprocess.Popen(command, cwd=cwd, env=environment, start_new_session=True)
    try:
        status = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        stop_process_group(process)
        raise BuildTimedOut(f"perturbed-KDF build exceeded {timeout:g}s") from error
    except BaseException:
        stop_process_group(process)
        raise
    if status != 0:
        raise subprocess.CalledProcessError(status, command)


def build(
    source: pathlib.Path, work: pathlib.Path, target: pathlib.Path, timeout: float
) -> pathlib.Path:
    source_path, original, changed = checked_source(source)
    refuse_unsafe_paths(source, work, target)
    work.parent.mkdir(parents=True, exist_ok=True)
    target.mkdir(parents=True, exist_ok=True)
    previous_handlers = {
        signum: signal.signal(signum, interrupted) for signum in (signal.SIGINT, signal.SIGTERM)
    }
    try:
        work.mkdir()
        sentinel = work / SENTINEL
        sentinel.write_text("disposable M-72 mutation source\n", encoding="utf-8")
        shutil.copytree(source, work, ignore=ignored, symlinks=True, dirs_exist_ok=True)
        copied_source = work / KDF_SOURCE
        if copied_source.read_bytes() != original:
            raise SystemExit("perturbed-KDF build: copied KDF source differs from the audited input")
        copied_source.write_bytes(changed)

        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(target)
        run_bounded_build(
            [
                environment.get("CARGO", "cargo"),
                "build",
                "-p",
                "sipx-call",
                "--example",
                "browser_audio_proof",
                "--features",
                "opus,dtls",
            ],
            work,
            environment,
            timeout,
        )
        binary = target / "debug/examples/browser_audio_proof"
        binary_bytes = binary.read_bytes()
        manifest = {
            "contract": "sipx.browser-audio.kdf-perturbation.v1",
            "mutation": "right-align-aead-master-salt-in-14-octet-x",
            "source": KDF_SOURCE.as_posix(),
            "original_sha256": digest(original),
            "perturbed_sha256": digest(changed),
            "binary_sha256": digest(binary_bytes),
        }
        sidecar = target / "debug/examples" / SIDECAR
        sidecar.write_text(json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
        return sidecar
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
        sentinel = work / SENTINEL
        if work.exists():
            if not sentinel.is_file():
                raise SystemExit(
                    f"perturbed-KDF build: refusing cleanup without owned-copy sentinel: {work}"
                )
            shutil.rmtree(work)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=pathlib.Path, default=ROOT)
    parser.add_argument("--work", type=pathlib.Path)
    parser.add_argument("--target", type=pathlib.Path)
    parser.add_argument("--build-timeout", type=float, default=DEFAULT_BUILD_TIMEOUT)
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    source = arguments.source.resolve()
    source_path, original, changed = checked_source(source)
    if arguments.check:
        print(
            json.dumps(
                {
                    "source": source_path.relative_to(source).as_posix(),
                    "original_sha256": digest(original),
                    "perturbed_sha256": digest(changed),
                },
                separators=(",", ":"),
            )
        )
        return 0
    if arguments.work is None or arguments.target is None:
        parser.error("--work and --target are required unless --check is used")
    sidecar = build(
        source,
        arguments.work.resolve(),
        arguments.target.resolve(),
        arguments.build_timeout,
    )
    print(sidecar)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
