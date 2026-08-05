#!/usr/bin/env python3
"""Hold the public CLI reference against executable help and versioned JSON producers.

The help half executes the binary. Reading Rust constants would compare one copy of the help with
another while a dispatch or build defect remained invisible. The JSON half discovers versioned
contracts and their literal structural fields beside the producers; a schema added to the command
cannot remain absent from the public inventory.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import re
import subprocess
import sys
from collections.abc import Mapping, Sequence, Set


ROOT = pathlib.Path(__file__).resolve().parent.parent
DOCUMENT = ROOT / "website" / "docs" / "reference" / "cli.md"
CLI_SOURCE = ROOT / "crates" / "sipx-cli" / "src"
APP_EVENT = ROOT / "crates" / "sipx-app-protocol" / "src" / "event.rs"

BEGIN_JSON = "<!-- BEGIN cli-json-contracts -->"
END_JSON = "<!-- END cli-json-contracts -->"

META_COMMANDS = {"help", "version"}
GLOBAL_OPTIONS = {"--help", "--json"}
BUILD_TIMEOUT_SECONDS = 600
HELP_TIMEOUT_SECONDS = 10


@dataclasses.dataclass(frozen=True)
class JsonContract:
    producer: str
    fields: Set[str]


def help_commands(text: str) -> set[str]:
    """Read the executable command names from the root help's COMMANDS block."""

    match = re.search(r"^COMMANDS:\s*$\n(?P<body>.*?)(?=^\S|\Z)", text, re.MULTILINE | re.DOTALL)
    if match is None:
        return set()
    return {
        name
        for name in re.findall(r"^\s{2,}([a-z][a-z0-9-]*)\s+", match.group("body"), re.MULTILINE)
        if name not in META_COMMANDS
    }


def help_options(text: str) -> set[str]:
    """Normalize every long option named by a command's executable help."""

    return set(re.findall(r"(?<![a-z0-9-])--[a-z][a-z0-9-]*", text)) - GLOBAL_OPTIONS


def document_command_flags(document: str) -> dict[str, set[str]]:
    """Read command sections and their option-table first cells from the public page."""

    headings = list(
        re.finditer(r"^## `sipx ([a-z][a-z0-9-]*)(?:\s+[^`]*)?`\s*$", document, re.MULTILINE)
    )
    commands: dict[str, set[str]] = {}
    for index, heading in enumerate(headings):
        end = headings[index + 1].start() if index + 1 < len(headings) else len(document)
        section = document[heading.end() : end]
        flags: set[str] = set()
        for line in section.splitlines():
            if not line.startswith("|"):
                continue
            first_cell = line.strip().strip("|").split("|", 1)[0]
            flags.update(re.findall(r"`(--[a-z][a-z0-9-]*)", first_cell))
        commands[heading.group(1)] = flags - GLOBAL_OPTIONS
    return commands


def help_drift(document: str, root_help: str, command_help: Mapping[str, str]) -> list[str]:
    """Describe command/option disagreement between executable help and the public page."""

    problems: list[str] = []
    executable_commands = help_commands(root_help)
    documented = document_command_flags(document)
    documented_commands = set(documented)
    for command in sorted(executable_commands - documented_commands):
        problems.append(f"executable command `{command}` has no public CLI-reference section")
    for command in sorted(documented_commands - executable_commands):
        problems.append(f"documented command `{command}` is absent from executable root help")
    for command in sorted(executable_commands & documented_commands):
        actual = help_options(command_help.get(command, ""))
        public = documented[command]
        for option in sorted(actual - public):
            problems.append(f"{command}: executable option `{option}` is not documented")
        for option in sorted(public - actual):
            problems.append(f"{command}: documented option `{option}` is absent from executable help")
    return problems


def _document_json_contracts(document: str) -> dict[str, JsonContract]:
    try:
        body = document.split(BEGIN_JSON, 1)[1].split(END_JSON, 1)[0]
    except IndexError:
        return {}
    contracts: dict[str, JsonContract] = {}
    for line in body.splitlines():
        if not line.startswith("|") or "---" in line:
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 3:
            continue
        contract_match = re.fullmatch(r"`(sipx\.[a-z0-9.-]+\.v[0-9]+)`", cells[0])
        producer_match = re.fullmatch(r"`([a-z][a-z0-9_-]*)`", cells[1])
        if contract_match is None or producer_match is None:
            continue
        contracts[contract_match.group(1)] = JsonContract(
            producer_match.group(1), frozenset(re.findall(r"`([a-z][a-z0-9_]*)`", cells[2]))
        )
    return contracts


def _literal_json_fields(source: str) -> frozenset[str]:
    """Fields structurally emitted by `json!` objects or inserted into their maps."""

    colon_fields = re.findall(r'"([a-z][a-z0-9_]*)"\s*:', source)
    inserted_fields = re.findall(r'\.insert\(\s*"([a-z][a-z0-9_]*)"', source)
    return frozenset(colon_fields + inserted_fields)


def discover_json_contracts(root: pathlib.Path) -> dict[str, JsonContract]:
    """Discover every versioned CLI schema/envelope and its literal structural fields."""

    cli_source = root / "crates" / "sipx-cli" / "src"
    contracts: dict[str, JsonContract] = {}
    version = re.compile(r'"(sipx\.[a-z0-9.-]+\.v[0-9]+)"')
    for path in sorted(cli_source.glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        # Test fixtures are consumers, not producers. Letting one introduce a field or a pretend
        # version would make the public table document its own test data instead of shipped output.
        source = re.split(r"^\s*#\[cfg\(test\)\]", source, maxsplit=1, flags=re.MULTILINE)[0]
        for name in version.findall(source):
            contract = JsonContract(path.stem, _literal_json_fields(source))
            previous = contracts.get(name)
            if previous is not None and previous != contract:
                raise ValueError(f"{name} is produced by both {previous.producer} and {path.stem}")
            contracts[name] = contract

    scenario = (cli_source / "scenario.rs").read_text(encoding="utf-8")
    scenario = re.split(r"^\s*#\[cfg\(test\)\]", scenario, maxsplit=1, flags=re.MULTILINE)[0]
    if re.search(r"\bsipx_app_protocol::\{[^}]*\bCONTRACT\b", scenario):
        event_source = (root / "crates" / "sipx-app-protocol" / "src" / "event.rs").read_text(
            encoding="utf-8"
        )
        match = re.search(r'pub const CONTRACT:\s*&str\s*=\s*"([^"]+)"', event_source)
        if match is None:
            raise ValueError("scenario imports CONTRACT, but sipx-app-protocol declares no value")
        contracts[match.group(1)] = JsonContract("scenario", _literal_json_fields(scenario))
    return contracts


def json_drift(document: str, actual: Mapping[str, JsonContract]) -> list[str]:
    """Describe disagreement between versioned producers and the checked public table."""

    public = _document_json_contracts(document)
    problems: list[str] = []
    for name in sorted(set(actual) - set(public)):
        problems.append(f"versioned CLI contract `{name}` is not documented")
    for name in sorted(set(public) - set(actual)):
        problems.append(f"documented CLI contract `{name}` has no producer")
    for name in sorted(set(actual) & set(public)):
        produced = actual[name]
        documented = public[name]
        if produced.producer != documented.producer:
            problems.append(
                f"{name}: producer is `{produced.producer}`, documented as `{documented.producer}`"
            )
        for field in sorted(produced.fields - documented.fields):
            problems.append(f"{name}: emitted field `{field}` is not documented")
        for field in sorted(documented.fields - produced.fields):
            problems.append(f"{name}: documented field `{field}` is not emitted structurally")
    return problems


def _run(command: Sequence[str], *, timeout: int) -> str:
    completed = subprocess.run(
        command,
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(f"{' '.join(command)} exited {completed.returncode}: {detail}")
    return completed.stdout


def build_binary() -> pathlib.Path:
    _run(("cargo", "build", "-p", "sipx-cli", "--quiet"), timeout=BUILD_TIMEOUT_SECONDS)
    metadata = json.loads(
        _run(("cargo", "metadata", "--no-deps", "--format-version", "1"), timeout=HELP_TIMEOUT_SECONDS)
    )
    suffix = ".exe" if os.name == "nt" else ""
    return pathlib.Path(metadata["target_directory"]) / "debug" / f"sipx{suffix}"


def executable_help(binary: pathlib.Path) -> tuple[str, dict[str, str]]:
    root_help = _run((str(binary), "--help"), timeout=HELP_TIMEOUT_SECONDS)
    commands = help_commands(root_help)
    return root_help, {
        command: _run((str(binary), command, "--help"), timeout=HELP_TIMEOUT_SECONDS)
        for command in sorted(commands)
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="check the public reference")
    parser.add_argument("--binary", type=pathlib.Path, help="use this already-built sipx binary")
    args = parser.parse_args(argv)

    document = DOCUMENT.read_text(encoding="utf-8")
    binary = args.binary or build_binary()
    try:
        root_help, commands = executable_help(binary)
        problems = help_drift(document, root_help, commands)
        problems.extend(json_drift(document, discover_json_contracts(ROOT)))
    except (OSError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
        print(f"cli reference: {error}", file=sys.stderr)
        return 1
    if problems:
        for problem in problems:
            print(f"cli reference: {problem}", file=sys.stderr)
        return 1
    print(
        f"cli reference: {len(commands)} command helps and "
        f"{len(discover_json_contracts(ROOT))} versioned JSON contracts agree"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
