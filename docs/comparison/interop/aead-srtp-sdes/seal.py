#!/usr/bin/env python3
"""Seal an exact-suite SDES evidence run with current source and artifact hashes."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import pathlib


CASE = pathlib.Path(__file__).resolve().parent
ROOT = CASE.parents[3]
SOURCE_FILES = (
    "crates/sipx-rtp/src/srtp/mod.rs",
    "crates/sipx-sdp/src/crypto.rs",
    "crates/sipx-call/src/media_policy.rs",
    "crates/sipx-call/src/call/mod.rs",
    "crates/sipx-call/src/call/offer_answer.rs",
    "crates/sipx-media/src/lib.rs",
    "crates/sipx-media/src/session.rs",
)
STATIC_FILES = (
    "adapter/Cargo.toml",
    "adapter/Cargo.lock",
    "adapter/src/main.rs",
    "peer/config",
    "peer/accounts",
    "bounded.py",
    "build-negative.py",
    "reproduce.sh",
    "seal.py",
)


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("run", type=pathlib.Path)
    parser.add_argument("normal_binary")
    parser.add_argument("perturbed_binary")
    args = parser.parse_args()
    run = args.run.resolve()
    if run.parent != CASE / "runs":
        raise SystemExit("run directory must be under this case's runs directory")
    relative_run = run.relative_to(CASE)
    evidence = (
        "AEAD_AES_128_GCM.json",
        "AEAD_AES_256_GCM.json",
        "KdfPerturbationSdes.json",
        "KdfPerturbationSdes-result.json",
        "peer-AEAD_AES_128_GCM.log",
        "peer-AEAD_AES_256_GCM.log",
        "peer-KdfPerturbationSdes.log",
    )
    hashes = {name: digest(CASE / name) for name in STATIC_FILES}
    hashes.update({f"{relative_run}/{name}": digest(run / name) for name in evidence})
    # A fresh measurement must retain the audit trail of earlier runs. The checker
    # inventories all evidence under the case; manifests are excluded to avoid
    # self-referential hashes. Only the fresh run supplies the result pointers below.
    hashes.update({
        path.relative_to(CASE).as_posix(): digest(path)
        for path in (CASE / "runs").rglob("*")
        if path.is_file() and path.name != "manifest.json"
    })
    source_hashes = {name: digest(ROOT / name) for name in SOURCE_FILES}
    manifest = {
        "schema": "sipx.comparison.interop.run.v1",
        "case": "aead-srtp-sdes",
        "subject": "baresip-v1",
        "evaluated_at": datetime.date.fromisoformat(run.name).isoformat(),
        "protocol": {
            "signalling": "TLS",
            "keying": "SDES",
            "media": "RTP/SAVP",
            "codec": "PCMU/8000/1",
        },
        "source_under_test": {
            "files": source_hashes,
            "normal_binary_sha256": args.normal_binary,
            "perturbed_binary_sha256": args.perturbed_binary,
        },
        "positive": {
            suite: {
                "result": f"{relative_run}/{suite}.json",
                "peer_log": f"{relative_run}/peer-{suite}.log",
            }
            for suite in ("AEAD_AES_128_GCM", "AEAD_AES_256_GCM")
        },
        "negative": {
            "suite": "AEAD_AES_256_GCM",
            "mutation": "right-align-aead-master-salt-in-14-octet-x",
            "build_manifest": f"{relative_run}/KdfPerturbationSdes.json",
            "result": f"{relative_run}/KdfPerturbationSdes-result.json",
            "peer_log": f"{relative_run}/peer-KdfPerturbationSdes.log",
            "peer_srtp_rejections": 50,
        },
        "sha256": dict(sorted(hashes.items())),
    }
    (run / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
