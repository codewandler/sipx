#!/usr/bin/env python3
"""Validate and compile the registry-shaped Supported v1 endpoint consumer."""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Mapping, Sequence


ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "tests" / "v1-endpoint-consumer"
MANIFEST = ROOT / "Cargo.toml"
SIPX_DEPENDENCIES = ("sipx-call", "sipx-sip", "sipx-transport", "sipx-ua")
DIRECT_DEPENDENCIES = frozenset((*SIPX_DEPENDENCIES, "tokio"))
COMPILE_TIMEOUT_SECONDS = 300
NEGATIVES = {
    "answer_specific_policy.rs": "no `AnswerMediaPolicy` in the root",
    "lower_layer_turn.rs": "use of unresolved module or unlinked crate `sipx_media`",
}


def workspace_facts(root: pathlib.Path = ROOT) -> tuple[str, str]:
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    package = manifest["workspace"]["package"]
    return str(package["version"]), str(package["edition"])


def dependency_version(value: object) -> str | None:
    if isinstance(value, str):
        return value
    if isinstance(value, Mapping) and isinstance(value.get("version"), str):
        return str(value["version"])
    return None


def fixture_problems(root: pathlib.Path = ROOT) -> list[str]:
    version, edition = workspace_facts(root)
    try:
        manifest_text = (FIXTURE / "Cargo.toml").read_text(encoding="utf-8")
        source = (FIXTURE / "src" / "main.rs").read_text(encoding="utf-8")
        manifest = tomllib.loads(manifest_text)
    except (FileNotFoundError, tomllib.TOMLDecodeError) as error:
        return [f"consumer input is missing or invalid: {error}"]

    problems: list[str] = []
    package = manifest.get("package", {})
    if not isinstance(package, Mapping):
        problems.append("consumer manifest has no [package] table")
    else:
        if package.get("edition") != edition:
            problems.append(f"consumer edition must be {edition}")
        if package.get("version") != "0.0.0" or package.get("publish") is not False:
            problems.append("consumer package must be private version 0.0.0")

    dependencies = manifest.get("dependencies", {})
    if not isinstance(dependencies, Mapping):
        return problems + ["consumer manifest has no [dependencies] table"]
    declared = frozenset(str(name) for name in dependencies)
    if declared != DIRECT_DEPENDENCIES:
        problems.append(
            "consumer dependencies differ from its exact direct set: "
            + ", ".join(sorted(declared ^ DIRECT_DEPENDENCIES))
        )
    for name, value in dependencies.items():
        if isinstance(value, Mapping) and any(key in value for key in ("path", "git", "workspace")):
            problems.append(f"{name}: registry fixture cannot use path, Git or workspace")
    for name in SIPX_DEPENDENCIES:
        if dependency_version(dependencies.get(name)) != f"={version}":
            problems.append(f"{name}: dependency must use exact version ={version}")
    tokio = dependencies.get("tokio")
    tokio_features = tokio.get("features", []) if isinstance(tokio, Mapping) else []
    if dependency_version(tokio) != "1" or set(tokio_features) != {"macros", "rt-multi-thread"}:
        problems.append("tokio: dependency must select only macros and rt-multi-thread")

    imported = {
        name.replace("_", "-")
        for name in re.findall(r"^use\s+(sipx_[a-z0-9_]+)\b", source, re.MULTILINE)
    }
    if re.search(r"^use\s+tokio\b", source, re.MULTILINE):
        imported.add("tokio")
    if imported != declared:
        problems.append(
            "source imports differ from its direct dependencies: "
            + ", ".join(sorted(imported ^ declared))
        )
    for name in NEGATIVES:
        if not (FIXTURE / "negative" / name).is_file():
            problems.append(f"negative compile fixture is missing: {name}")
    return problems


def bounded(command: Sequence[str], *, cwd: pathlib.Path) -> subprocess.CompletedProcess[str]:
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=COMPILE_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.communicate(timeout=5)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.communicate()
        raise RuntimeError(
            f"consumer compile exceeded {COMPILE_TIMEOUT_SECONDS}s: {' '.join(command)}"
        ) from None
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def compile_consumer(root: pathlib.Path = ROOT) -> list[str]:
    problems = fixture_problems(root)
    if problems:
        return problems
    scratch = root / "target"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="v1-endpoint-consumer-", dir=scratch) as directory:
        project = pathlib.Path(directory) / "consumer"
        shutil.copytree(FIXTURE, project, ignore=shutil.ignore_patterns("negative"))
        # The disposable copy lives below this checkout's `target/`; an empty workspace keeps
        # Cargo from treating that path as an undeclared member of the parent workspace.
        patches = ["", "[workspace]", "", "[patch.crates-io]"]
        for name in SIPX_DEPENDENCIES:
            path = root / "crates" / name
            patches.append(f"{name} = {{ path = {json.dumps(str(path))} }}")
        with (project / "Cargo.toml").open("a", encoding="utf-8") as manifest:
            manifest.write("\n".join(patches) + "\n")

        checked = bounded(("cargo", "check", "--quiet"), cwd=project)
        if checked.returncode != 0:
            complaint = checked.stderr.strip() or checked.stdout.strip()
            return [f"Supported endpoint consumer did not compile: {complaint}"]

        for name, expected in NEGATIVES.items():
            source = FIXTURE / "negative" / name
            (project / "src" / "main.rs").write_text(
                source.read_text(encoding="utf-8"), encoding="utf-8"
            )
            failed = bounded(("cargo", "check", "--quiet"), cwd=project)
            if failed.returncode == 0:
                problems.append(f"negative compile fixture unexpectedly succeeded: {name}")
            elif expected not in failed.stderr:
                problems.append(f"{name}: failure did not contain {expected!r}: {failed.stderr.strip()}")
    return problems


def main() -> int:
    problems = compile_consumer()
    for problem in problems:
        print(f"v1 endpoint consumer: {problem}", file=sys.stderr)
    if problems:
        return 1
    print("v1 endpoint consumer: registry-shaped Supported path and negative fixtures pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
