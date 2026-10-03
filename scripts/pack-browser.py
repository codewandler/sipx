#!/usr/bin/env python3
"""Pack an explicit candidate; record committed or dirty source provenance honestly."""

import argparse, hashlib, json, os, pathlib, shutil, subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=pathlib.Path, required=True)
    args = p.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["python3", str(ROOT / "scripts/generate-browser.py"), "--check"], check=True
    )
    stage = out / "stage"
    stage.mkdir(exist_ok=False)
    subprocess.run(
        ["cargo", "build", "--release", "--target", "wasm32-unknown-unknown"],
        cwd=ROOT / "wasm",
        env={**os.environ, "RUSTFLAGS": "-C link-arg=--max-memory=33554432"},
        check=True,
    )
    source = ROOT / "browser"
    for name in ["src", "generated"]:
        shutil.copytree(source / name, stage / name)
    for name in ["package.json", "index.d.ts", "README.md"]:
        shutil.copyfile(source / name, stage / name)
    for name in ["LICENSE-MIT", "LICENSE-APACHE"]:
        shutil.copyfile(ROOT / name, stage / name)
    shutil.copyfile(
        ROOT / "wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm",
        stage / "sipx_browser.wasm",
    )
    files = (
        subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            cwd=ROOT,
        )
        .decode()
        .split("\0")
    )
    inputs = {
        name: digest(ROOT / name)
        for name in sorted(set(files))
        if name
        and (ROOT / name).is_file()
        and name.startswith(
            (
                "browser/",
                "crates/",
                "Cargo.",
                "wasm/",
                "scripts/generate-browser.py",
                "scripts/pack-browser.py",
            )
        )
    }
    head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    provenance = {
        "source": {
            "base_commit": head,
            "dirty": dirty,
            "source_sha256": hashlib.sha256(
                json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "files": inputs,
        },
        "wasm_sha256": digest(stage / "sipx_browser.wasm"),
        "experimental": True,
        "registry_published": False,
    }
    (stage / "PROVENANCE.json").write_text(json.dumps(provenance, indent=2) + "\n")
    sums = "".join(
        f"{digest(path)}  {path.relative_to(stage)}\n"
        for path in sorted(stage.rglob("*"))
        if path.is_file()
    )
    (stage / "SHA256SUMS").write_text(sums)
    packed_output = json.loads(
        subprocess.check_output(
            [
                "npm",
                "pack",
                "--json",
                "--ignore-scripts",
                "--pack-destination",
                str(out),
            ],
            cwd=stage,
            text=True,
        )
    )
    packed = (
        packed_output[0]
        if isinstance(packed_output, list)
        else packed_output["@sipx/browser"]
    )
    archive = out / packed["filename"]
    result = {
        "archive": str(archive),
        "sha256": digest(archive),
        "source": provenance["source"],
        "files": [x["path"] for x in packed["files"]],
    }
    (out / "artifact.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "source"}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
