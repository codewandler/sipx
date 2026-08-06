#!/usr/bin/env python3
"""Compile the published answer consumer and verify its rendered onboarding sentence."""

from __future__ import annotations

import argparse
import html.parser
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
FIXTURE = ROOT / "tests" / "published-answer-consumer"
EXAMPLE = ROOT / "crates" / "sipx-call" / "examples" / "answer_a_call.rs"
MANIFEST = ROOT / "Cargo.toml"
BUILT_PAGE = ROOT / "website" / "build" / "docs" / "getting-started.html"
SIPX_DEPENDENCIES = ("sipx-call", "sipx-sip", "sipx-transport")
DIRECT_DEPENDENCIES = frozenset((*SIPX_DEPENDENCIES, "tokio"))
COMPILE_TIMEOUT_SECONDS = 300


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


def imported_packages(source: str) -> frozenset[str]:
    packages = {
        name.replace("_", "-")
        for name in re.findall(r"^use\s+(sipx_[a-z0-9_]+)\b", source, re.MULTILINE)
    }
    if re.search(r"#\[tokio::[a-z_]+\]", source):
        packages.add("tokio")
    return frozenset(packages)


def source_problems(
    manifest_text: str,
    source: str,
    example: str,
    *,
    version: str,
    edition: str,
) -> list[str]:
    """Validate the archived consumer without consulting a workspace dependency graph."""

    problems: list[str] = []
    try:
        manifest = tomllib.loads(manifest_text)
    except tomllib.TOMLDecodeError as error:
        return [f"consumer manifest is invalid TOML: {error}"]
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
        missing = sorted(DIRECT_DEPENDENCIES - declared)
        extra = sorted(declared - DIRECT_DEPENDENCIES)
        if missing:
            problems.append("consumer dependencies missing: " + ", ".join(missing))
        if extra:
            problems.append("consumer dependencies are not minimal: " + ", ".join(extra))
    for name, value in dependencies.items():
        if isinstance(value, Mapping) and any(key in value for key in ("path", "git", "workspace")):
            problems.append(f"{name}: published dependency cannot use path, Git or workspace")
    for name in SIPX_DEPENDENCIES:
        if dependency_version(dependencies.get(name)) != f"={version}":
            problems.append(f"{name}: dependency must use exact version ={version}")
    tokio = dependencies.get("tokio")
    tokio_features = tokio.get("features", []) if isinstance(tokio, Mapping) else []
    if dependency_version(tokio) != "1" or set(tokio_features) != {"macros", "rt-multi-thread"}:
        problems.append("tokio: dependency must select only macros and rt-multi-thread")

    imports = imported_packages(source)
    if imports != declared:
        undeclared = sorted(imports - declared)
        unused = sorted(declared - imports)
        if undeclared:
            problems.append("source imports undeclared packages: " + ", ".join(undeclared))
        if unused:
            problems.append("manifest declares packages absent from source: " + ", ".join(unused))
    if source != example:
        problems.append("archived consumer source differs from answer_a_call.rs")
    return problems


def fixture_problems(root: pathlib.Path = ROOT) -> list[str]:
    version, edition = workspace_facts(root)
    fixture = root / "tests" / "published-answer-consumer"
    example = root / "crates" / "sipx-call" / "examples" / "answer_a_call.rs"
    try:
        return source_problems(
            (fixture / "Cargo.toml").read_text(encoding="utf-8"),
            (fixture / "src" / "main.rs").read_text(encoding="utf-8"),
            example.read_text(encoding="utf-8"),
            version=version,
            edition=edition,
        )
    except FileNotFoundError as error:
        return [f"published consumer input is missing: {error.filename}"]


def _bounded(command: Sequence[str], *, cwd: pathlib.Path) -> subprocess.CompletedProcess[str]:
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
    """Compile a clean copy; local patches never enter the archived/displayed manifest."""

    problems = fixture_problems(root)
    if problems:
        return problems
    fixture = root / "tests" / "published-answer-consumer"
    with tempfile.TemporaryDirectory(prefix="sipx-published-answer-") as directory:
        project = pathlib.Path(directory) / "consumer"
        shutil.copytree(fixture, project)
        patches = ["", "[patch.crates-io]"]
        for name in SIPX_DEPENDENCIES:
            path = root / "crates" / name
            patches.append(f"{name} = {{ path = {json.dumps(str(path))} }}")
        with (project / "Cargo.toml").open("a", encoding="utf-8") as manifest:
            manifest.write("\n".join(patches) + "\n")
        checked = _bounded(("cargo", "check", "--quiet"), cwd=project)
        if checked.returncode != 0:
            complaint = checked.stderr.strip() or checked.stdout.strip()
            return [f"published answer consumer did not compile: {complaint}"]
    return []


class Paragraphs(html.parser.HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.current: list[str] | None = None
        self.paragraphs: list[str] = []
        self.hidden = 0

    def handle_starttag(self, tag: str, _attrs: list[tuple[str, str | None]]) -> None:
        if tag in {"script", "style", "template"}:
            self.hidden += 1
        elif tag == "p" and self.hidden == 0:
            self.current = []

    def handle_endtag(self, tag: str) -> None:
        if tag in {"script", "style", "template"} and self.hidden:
            self.hidden -= 1
        elif tag == "p" and self.current is not None:
            self.paragraphs.append(" ".join("".join(self.current).split()))
            self.current = None

    def handle_data(self, data: str) -> None:
        if self.hidden == 0 and self.current is not None:
            self.current.append(data)


def built_page_problems(html: str, version: str) -> list[str]:
    parsed = Paragraphs()
    parsed.feed(html)
    expected = (
        "Confirm which version was installed. This documentation build covers "
        f"{version}:"
    )
    if expected not in parsed.paragraphs:
        return [f"built getting-started page has no complete visible version sentence: {expected}"]
    return []


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="validate and compile the clean consumer")
    mode.add_argument("--built", action="store_true", help="verify the built getting-started HTML")
    args = parser.parse_args(argv)
    if args.check:
        problems = compile_consumer()
        success = "published answer consumer is registry-shaped and compiles"
    else:
        version, _edition = workspace_facts()
        try:
            problems = built_page_problems(BUILT_PAGE.read_text(encoding="utf-8"), version)
        except FileNotFoundError:
            problems = [f"built getting-started page is missing: {BUILT_PAGE}"]
        success = "built getting-started page contains the complete version sentence"
    for problem in problems:
        print(f"published onboarding: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(f"published onboarding: {success}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
