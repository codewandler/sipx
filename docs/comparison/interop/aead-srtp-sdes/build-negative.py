#!/usr/bin/env python3
"""Build the SDES probe against an isolated, deliberately incorrect AEAD KDF."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys


CASE = pathlib.Path(__file__).resolve().parent
ROOT = CASE.parents[3]
KDF_SOURCE = pathlib.Path("crates/sipx-rtp/src/srtp/mod.rs")
SENTINEL = ".owned-m72-negative-source"
ORIGINAL = """    let slot = iv
        .get_mut(..master_salt.len())
        .ok_or(SrtpError::KeyLength {
"""
PERTURBED = """    // Disposable M-72 negative: deliberately use the wrong AEAD salt alignment.
    let salt_offset = MASTER_SALT_LEN.saturating_sub(master_salt.len());
    let salt_end = salt_offset.saturating_add(master_salt.len());
    let slot = iv
        .get_mut(salt_offset..salt_end)
        .ok_or(SrtpError::KeyLength {
"""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def ignored(_directory: str, names: list[str]) -> set[str]:
    return set(names).intersection({".git", ".claude", "node_modules", "target", "__pycache__"})


def stop_group(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    process.wait()


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: build-negative.py WORK TARGET MANIFEST")
    work = pathlib.Path(sys.argv[1]).resolve()
    target = pathlib.Path(sys.argv[2]).resolve()
    output = pathlib.Path(sys.argv[3]).resolve()
    if ROOT not in work.parents or ROOT not in target.parents:
        raise SystemExit("work and target must be inside this checkout")
    if work.exists():
        raise SystemExit(f"owned disposable source already exists: {work}")
    target.mkdir(parents=True, exist_ok=True)
    original_path = ROOT / KDF_SOURCE
    original = original_path.read_bytes()
    original_text = original.decode("utf-8")
    if original_text.count(ORIGINAL) != 1 or PERTURBED in original_text:
        raise SystemExit("the audited KDF source fragment is not present exactly once")
    perturbed = original_text.replace(ORIGINAL, PERTURBED, 1).encode("utf-8")

    process: subprocess.Popen[bytes] | None = None
    try:
        work.mkdir()
        (work / SENTINEL).write_text("owned disposable M-72 source\n", encoding="utf-8")
        shutil.copytree(ROOT, work, ignore=ignored, symlinks=True, dirs_exist_ok=True)
        copied_kdf = work / KDF_SOURCE
        if copied_kdf.read_bytes() != original:
            raise SystemExit("copied KDF source differs from audited source")
        copied_kdf.write_bytes(perturbed)
        shutil.copytree(CASE / "adapter", work / "adapter")
        manifest_path = work / "adapter/Cargo.toml"
        manifest_text = manifest_path.read_text(encoding="utf-8")
        manifest_path.write_text(
            manifest_text.replace('../../../../../crates/', '../crates/'), encoding="utf-8"
        )

        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(target)
        environment.setdefault("RUSTC_WRAPPER", "sccache")
        process = subprocess.Popen(
            ["cargo", "build", "--manifest-path", "adapter/Cargo.toml"],
            cwd=work,
            env=environment,
            start_new_session=True,
        )
        try:
            status = process.wait(timeout=600)
        except subprocess.TimeoutExpired as error:
            stop_group(process)
            raise SystemExit("negative probe build exceeded 600 seconds") from error
        if status != 0:
            raise subprocess.CalledProcessError(status, process.args)
        binary = target / "debug/m72-sdes-proof"
        build_manifest = {
            "contract": "sipx.m72.sdes-kdf-perturbation.v1",
            "mutation": "right-align-aead-master-salt-in-14-octet-x",
            "source": KDF_SOURCE.as_posix(),
            "original_sha256": sha256(original),
            "perturbed_sha256": sha256(perturbed),
            "binary_sha256": sha256(binary.read_bytes()),
        }
        output.write_text(
            json.dumps(build_manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(binary)
        return 0
    finally:
        if process is not None and process.poll() is None:
            stop_group(process)
        if work.exists():
            if not (work / SENTINEL).is_file():
                raise SystemExit(f"refusing cleanup without owned-copy sentinel: {work}")
            shutil.rmtree(work)


if __name__ == "__main__":
    raise SystemExit(main())
