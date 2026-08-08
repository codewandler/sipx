#!/usr/bin/env python3
"""Hold each command's result records against one another, so no outcome answers fewer questions.

`P-25` gave `sipx register`'s timeout record an `aor` and left its rejection and transport-failure
records without one, because widening the diff was out of scope. The result was a field a script
could read on two of four outcomes: matching on *which* registration a record described meant
branching on success first, which is a shape no consumer should have to learn. `P-28` closed that
instance. This closes the class — a field that appears on one outcome and not its siblings is a
finding here before it is a surprise in somebody's pipeline.

**The field set is derived, not listed.** Every command's records are read out of the
`Report::new()` builder chains in its module under `crates/sipx-cli/src/`, and compared with each
other. Nothing about the expected fields is written down: adding a field to one outcome makes this
red on the commit that adds it, whether or not anybody remembered this file existed.

**What cannot be derived is written down once, next to the check.** Some fields genuinely belong
to one outcome — a registrar's lease exists only where a registrar answered, and an error string
only where something failed. Those are judgement, not data, so they live in `OUTCOME_SPECIFIC`
below, each with the sentence that justifies it, and a record that exists before its command has
a subject at all is named in `WITHOUT_A_CALL`. That is the arrangement `AGENTS.md` gives for
`COMPARISON_SCOPE`: widening the exemption is a reviewable diff rather than a re-reading of a
paragraph. A field not named there must appear on every outcome of its command or this is red.
An exemption nothing needs any more is red too, so the table cannot outlive its reasons.

**And the public reference stays the enumeration.** Every field a command reports must be named on
`website/docs/reference/cli.md`, so the page is where a consumer learns the contract rather than a
place it is described. That is the half of `P-28`'s fourth row a script can hold: whether a new
field also earned a `CHANGELOG.md` entry is a judgement about significance and is not checked here.

## What it reads, exactly

* Modules: `crates/sipx-cli/src/*.rs`, minus `output.rs` — that is the builder's own module and
  its `Report`s are the type's examples, not any command's results.
* Each module is truncated at its first `#[cfg(test)]`. A test fixture is a consumer of the
  report shape, and one that built a lopsided `Report` on purpose would otherwise fail this.
* A **record** is one `Report::new()` builder chain. A chain that names a `status` field is an
  **outcome** — `P-1` set the form that every result line names its status — and one that does
  not is a **contributor**: a helper assembling a fragment other reports are given, which has no
  siblings to be compared with.
* A record's fields are the literal first arguments of the chain's `text`/`number`/`boolean`/
  `seconds`/`millis`/`decimal`/`list` calls.

## What it cannot see, exactly

This is a text reader, not a compiler, and three constructions are invisible to it. Each one is
*counted* rather than merely disclaimed — the summary line prints how many field additions it
could not attribute to an outcome, and `--explain` names them with their file and line, so the
blind spot is visible in the gate's own output instead of only in this comment.

1. **Fields added through a binding**, `report = report.boolean("flow", …)`, which is how a
   conditional field is written. Attributing one to the chain it follows would need the flow
   analysis this deliberately does not do, and attributing it wrongly is worse than not at all.
2. **Fields added by a helper the report is passed to** — `transport.report(…)`,
   `crate::destination::with_attempts(…)`, `export.into_report(…)`. Two types in this crate have
   a method called `report`, so resolving a call by its name would sometimes attribute the wrong
   fields, and a check that is confidently wrong is worse than one that says what it skipped.
3. **Fields whose name is not a string literal.** There are none today; a `format!`-built field
   name would be counted as unattributed rather than silently dropped.

The consequence is stated plainly: this proves parity over the fields a command's outcomes name
directly, and an imbalance introduced *only* through one of the three constructions above is not
caught here. It over-reports nothing — every field it compares is one the chain names itself.

Usage:
    ./scripts/check-outcome-parity.py --check      # the gate's mode
    ./scripts/check-outcome-parity.py --explain    # print every record this derived
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
from typing import NamedTuple

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Where the commands live. One module per command, which is what makes the module name the
#: command name and the sibling records in it the outcomes to compare.
COMMANDS = pathlib.PurePath("crates/sipx-cli/src")

#: The builder's own module. Its `Report`s demonstrate the type; they are nobody's result.
BUILDER = "output.rs"

#: The public page that enumerates what each command reports. A field a command emits and this
#: page does not name is a field somebody has to read the source to discover, which is the same
#: failure as an outcome that omits it — one more thing a consumer cannot learn from the contract.
REFERENCE = pathlib.PurePath("website/docs/reference/cli.md")

#: The builder methods that name a field. `render`, `emit` and `names` take no field name and
#: end a chain, which is why the set is spelled out rather than "any method call".
FIELD_METHODS = ("text", "number", "boolean", "seconds", "millis", "decimal", "list")

#: The field whose presence makes a record a result rather than a fragment (`P-1`).
STATUS = "status"

#: Below this the reader has stopped understanding the crate rather than found a small CLI. A
#: reader that silently finds nothing reports perfect parity, which is indistinguishable from a
#: repository where every command agrees — the exact failure this file exists to make impossible.
#: Deliberately floors and not the current counts: `scripts/test-outcome-parity.py` pins what the
#: crate holds today, and a checker that had to be edited whenever a command gained an outcome
#: would be edited without being read.
PLAUSIBLE_COMMANDS = 2
PLAUSIBLE_OUTCOMES = 6

#: Fields a command's outcomes are allowed to disagree about, each with why. Keyed by field name
#: rather than by command: these are properties of the *fact* a field reports, and a fact that
#: only exists on one kind of ending exists that way wherever it is reported.
#:
#: Nothing about identity belongs here. `aor`, `peer`, `caller` and their kind name *which* thing
#: a record is about, are known before the exchange begins, and are therefore reportable whatever
#: happened — which is the whole of `P-28`.
OUTCOME_SPECIFIC: dict[str, str] = {
    "error": "what went wrong exists only where something did; a successful record carrying an "
    "empty `error` would be read as a failure with no message",
    "expires": "the lease a registrar granted, so it exists only where a registrar answered",
    "refresh_in": "derived from the granted lease, and absent for the same reason",
    "registration_limit_ms": "the deadline is reported by the record that hit it; on every other "
    "ending the limit was not what decided the outcome",
    "registration_elapsed_ms": "measured against that deadline, and reported beside it",
    "cleanup_ms": "how long the join took after the deadline expired, which only the timeout "
    "record performs and can therefore measure",
    "ended_by": "the cause a *call* ended for, which a record written before the call was ever "
    "established has nothing to name",
    "duration_ms": "how long a call ran, so it exists only where one ran",
    "samples_recorded": "the media that flowed, absent where no media session was established",
    "heard_audio": "the same measurement's headline, absent for the same reason",
    "media_advertised": "the address offered in the SDP, so it exists only where an offer or "
    "answer was actually made",
    "media_bound": "the socket the media session bound, absent where none was created",
    "code": "the status code this command *sent*, which only the refusing record sends",
}

#: Records written before the command has a subject, keyed by `(command, status)`.
#:
#: This is the strongest exemption in the file — it removes a whole record from the comparison
#: rather than one field — so it is the one to be most sceptical of, and both entries state a
#: fact about *where the record is emitted from* that a reader can check in one jump. A record
#: reached before any call exists cannot name a caller, and requiring it to would mean printing
#: the listener's own address under a field a consumer reads as the far end.
#:
#: `answer` is the only command with such records, because it is the only one whose subject
#: arrives from the network rather than from the command line. `dial` and `register` are told
#: what they are about before they open a socket, which is why neither appears here.
WITHOUT_A_CALL: dict[tuple[str, str], str] = {
    ("answer", "listening"): "printed once the listener is bound and before `incoming.recv()` has "
    "produced anything, so it can name nothing a call decides — the call's own record follows it "
    "on the same stream",
    ("answer", "interrupted"): "reached only from the select that waits for the first INVITE, so "
    "the command was stopped before a caller existed. An interrupt *during* a call is reported by "
    "the terminal record instead, which carries `caller` like every other ending",
}


class Field(NamedTuple):
    """One named field of one record."""

    name: str
    line: int


class Record(NamedTuple):
    """One `Report::new()` chain: a command's result, or a fragment other results are given."""

    command: str
    #: The literal `status` value, when the chain writes one. `""` for a fragment, and for a
    #: record whose status is a computed `Exit`, which is labelled by its position instead.
    status: str
    line: int
    fields: tuple[Field, ...]

    @property
    def is_outcome(self) -> bool:
        """A record that names a status is a result line; one that does not is a fragment."""
        return any(field.name == STATUS for field in self.fields)

    @property
    def label(self) -> str:
        """What to call this outcome in a finding."""
        return self.status or f"{self.command}.rs:{self.line}"


class Unattributed(NamedTuple):
    """A field addition this reader could not tie to one record. The blind spot, counted."""

    command: str
    name: str
    line: int


# ------------------------------------------------------------------------------------------------
# Reading Rust without parsing it
# ------------------------------------------------------------------------------------------------

_IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_STRING = re.compile(r'\s*"((?:[^"\\]|\\.)*)"')
_CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]", re.MULTILINE)
_FIELD_CALL = re.compile(rf"\.(?:{'|'.join(FIELD_METHODS)})\s*\(")


def command_source(text: str) -> str:
    """A module with its test module removed.

    A test builds `Report`s to assert on the builder, including deliberately lopsided ones. They
    are consumers of the shape rather than any command's output, and letting one in would mean
    this check reports on its own fixtures — the failure `check-cli-reference.py` names in the
    same words for the same reason.
    """
    return _CFG_TEST.split(text, maxsplit=1)[0]


def _skip_trivia(text: str, index: int) -> int:
    """Advance past whitespace and line comments, which sit between a chain's calls."""
    while index < len(text):
        if text[index].isspace():
            index += 1
            continue
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = len(text) if end == -1 else end + 1
            continue
        return index
    return index


def _call_arguments(text: str, open_paren: int) -> tuple[str, int]:
    """The text between a call's parentheses, and the index just past its close.

    Character by character rather than by regex, because an argument can contain parentheses,
    a string containing a parenthesis, and a `'a'` whose quote is not a string at all.
    """
    depth = 0
    index = open_paren
    while index < len(text):
        char = text[index]
        if char == '"':
            index += 1
            while index < len(text) and text[index] != '"':
                index += 2 if text[index] == "\\" else 1
            index += 1
            continue
        if char == "'":
            # A lifetime (`'a`) or a character literal (`'x'`, `'\n'`). Only the latter closes.
            closing = re.match(r"'(?:\\.|[^\\'])'", text[index:])
            index += closing.end() if closing else 1
            continue
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0:
                return text[open_paren + 1 : index], index + 1
        index += 1
    return text[open_paren + 1 :], len(text)


def _split_arguments(arguments: str) -> list[str]:
    """A call's arguments, split at the commas that separate them and not the ones inside them."""
    parts: list[str] = []
    depth = 0
    start = 0
    index = 0
    while index < len(arguments):
        char = arguments[index]
        if char == '"':
            index += 1
            while index < len(arguments) and arguments[index] != '"':
                index += 2 if arguments[index] == "\\" else 1
        elif char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(arguments[start:index])
            start = index + 1
        index += 1
    parts.append(arguments[start:])
    return [part.strip() for part in parts if part.strip()]


def _literal(argument: str) -> str:
    """The value of a string-literal argument, or `""` when it is anything else."""
    match = _STRING.fullmatch(argument)
    return match.group(1) if match else ""


def _status_value(argument: str) -> str:
    """The outcome a status argument names, when the source says so in one place.

    Two spellings reach a literal: `"registered"`, and `Exit::Timeout.as_str()`, whose variant is
    the name the process exits under. A computed `exit.as_str()` names no single outcome, and
    labelling such a record by its position is honest where guessing would not be.
    """
    literal = _literal(argument)
    if literal:
        return literal
    variant = re.fullmatch(r"Exit::(\w+)\.as_str\(\)", argument)
    return _snake(variant.group(1)) if variant else ""


def _snake(camel: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


def read_records(command: str, text: str) -> tuple[list[Record], list[Unattributed]]:
    """Every `Report::new()` chain in one module, and the field additions outside them."""
    records: list[Record] = []
    covered: list[tuple[int, int]] = []

    for start in (match.end() for match in re.finditer(r"Report::new\s*\(\s*\)", text)):
        fields: list[Field] = []
        status = ""
        index = start
        while True:
            index = _skip_trivia(text, index)
            if index >= len(text) or text[index] != ".":
                break
            name_match = _IDENTIFIER.match(text, index + 1)
            if name_match is None:
                break
            after = _skip_trivia(text, name_match.end())
            if after >= len(text) or text[after] != "(":
                break
            if name_match.group(0) not in FIELD_METHODS:
                break
            arguments, end = _call_arguments(text, after)
            parts = _split_arguments(arguments)
            field = _literal(parts[0]) if parts else ""
            if field:
                fields.append(Field(field, _line(text, index)))
                if field == STATUS and len(parts) > 1:
                    status = status or _status_value(parts[1])
            covered.append((index, end))
            index = end
        records.append(Record(command, status, _line(text, start), tuple(fields)))

    # Every remaining field-naming call: the ones a helper makes, and the ones made through a
    # binding. Counted rather than compared, and counted rather than ignored — the summary prints
    # the number so the reader of a green run knows the size of what was not compared.
    unattributed = [
        Unattributed(command, _first_name(text, call.end() - 1), _line(text, call.start()))
        for call in _FIELD_CALL.finditer(text)
        if not any(start <= call.start() < end for start, end in covered)
    ]
    return records, unattributed


def _first_name(text: str, open_paren: int) -> str:
    """The literal field name a call names, or `""` when it is computed."""
    parts = _split_arguments(_call_arguments(text, open_paren)[0])
    return _literal(parts[0]) if parts else ""


def _line(text: str, index: int) -> int:
    return text.count("\n", 0, index) + 1


def read_commands(root: pathlib.Path) -> tuple[dict[str, list[Record]], list[Unattributed]]:
    """Every command module's records, keyed by command name."""
    by_command: dict[str, list[Record]] = {}
    unattributed: list[Unattributed] = []
    for module in sorted((root / COMMANDS).glob("*.rs")):
        if module.name == BUILDER:
            continue
        records, skipped = read_records(
            module.stem, command_source(module.read_text(encoding="utf-8"))
        )
        if records:
            by_command[module.stem] = records
        unattributed.extend(skipped)
    return by_command, unattributed


# ------------------------------------------------------------------------------------------------
# The rule
# ------------------------------------------------------------------------------------------------


def outcomes(records: list[Record]) -> list[Record]:
    """The result lines among a module's records, the pre-call ones excluded."""
    return [
        record
        for record in records
        if record.is_outcome and (record.command, record.status) not in WITHOUT_A_CALL
    ]


def compared(by_command: dict[str, list[Record]]) -> dict[str, list[Record]]:
    """The commands whose outcomes have siblings to be held against.

    A command with one outcome is not evidence of parity and is not counted as any. Saying so is
    the difference between "four commands agree" and "three were actually compared".
    """
    return {
        command: siblings
        for command, records in by_command.items()
        if len(siblings := outcomes(records)) > 1
    }


def compared_fields(by_command: dict[str, list[Record]]) -> set[str]:
    """Every field that took part in a comparison, which is what the claim is about."""
    return {
        field.name
        for siblings in compared(by_command).values()
        for record in siblings
        for field in record.fields
    }


def parity_problems(by_command: dict[str, list[Record]]) -> list[str]:
    """Fields one outcome carries and a sibling does not, minus the declared exemptions."""
    problems: list[str] = []
    for command, siblings in sorted(compared(by_command).items()):
        carried: dict[str, list[str]] = {}
        for record in siblings:
            for field in record.fields:
                carried.setdefault(field.name, []).append(record.label)
        for field in sorted(carried):
            if field in OUTCOME_SPECIFIC:
                continue
            missing = [
                record.label
                for record in siblings
                if field not in {existing.name for existing in record.fields}
            ]
            if not missing:
                continue
            problems.append(
                f"{command}: `{field}` is reported by {_and(carried[field])} and not by "
                f"{_and(missing)}; a script reading it has to branch on the outcome first. "
                f"Report it there too, or declare it in OUTCOME_SPECIFIC with why it cannot be."
            )
    return problems


def _and(labels: list[str]) -> str:
    unique = sorted(set(labels))
    if len(unique) == 1:
        return f"`{unique[0]}`"
    return ", ".join(f"`{label}`" for label in unique[:-1]) + f" and `{unique[-1]}`"


def scope_problems(by_command: dict[str, list[Record]]) -> list[str]:
    """Whether the reader still understands the crate, or has quietly stopped reading it.

    Every previous silent checker in this repository failed this way — it observed less than it
    claimed and reported the emptiness as success. A reader that finds no commands, or only a
    handful of records where a CLI with this many subcommands must have more, has drifted from
    the source rather than found a tidy tree.
    """
    problems: list[str] = []
    siblings = compared(by_command)
    if not siblings:
        return [
            f"read no command with sibling outcomes at all from {COMMANDS}/*.rs; the reader has "
            f"drifted from the source and would report parity over nothing"
        ]
    if len(siblings) < PLAUSIBLE_COMMANDS:
        problems.append(
            f"read sibling outcomes for only {len(siblings)} command(s) "
            f"({', '.join(sorted(siblings))}); fewer than {PLAUSIBLE_COMMANDS} means the chain "
            f"reader no longer matches how {COMMANDS} builds reports"
        )
    total = sum(len(found) for found in siblings.values())
    if total < PLAUSIBLE_OUTCOMES:
        problems.append(
            f"read only {total} comparable outcome(s) across {len(siblings)} command(s); fewer "
            f"than {PLAUSIBLE_OUTCOMES} means the chain reader is finding chains and not fields"
        )
    return problems


def documentation_problems(by_command: dict[str, list[Record]], root: pathlib.Path) -> list[str]:
    """Fields a command reports that the public reference does not name.

    One direction only. A field emitted and not documented is a contract a consumer can discover
    solely by reading Rust; a field documented and not found here is usually one of the helper
    contributions this reader cannot see, so requiring the reverse would fail on the very blind
    spot the header declares. `check-cli-reference.py` owns the versioned `sipx.*.vN` schemas and
    holds them in both directions — this covers the report fields, which are not in that table.
    """
    page = root / REFERENCE
    if not page.is_file():
        # A fabricated tree assembled by a test has no website. Silence about a page that does not
        # exist is not a finding, and pretending otherwise would make every fixture fail here.
        return []
    text = page.read_text(encoding="utf-8")
    documented = set(re.findall(r"`([a-z][a-z0-9_]*)`", text))
    emitted = {
        (field.name, record.command)
        for records in by_command.values()
        for record in records
        if record.is_outcome
        for field in record.fields
    }
    return [
        f"{command} reports `{field}` and {REFERENCE} does not name it; a consumer would have to "
        f"read the source to find out it exists"
        for field, command in sorted(emitted)
        if field not in documented
    ]


def unused_exemptions(by_command: dict[str, list[Record]]) -> list[str]:
    """Declared exemptions nothing needs any more, so the table cannot outlive its reasons.

    Held against the fields that actually take part in a comparison, not against every field in
    the crate. An entry excusing a field nothing compares is a reason nobody can check, and a
    table of those is how the next reader learns to skim this one.
    """
    participating = compared_fields(by_command)
    pairs = {
        (record.command, record.status)
        for records in by_command.values()
        for record in records
        if record.is_outcome
    }
    return [
        f"OUTCOME_SPECIFIC declares `{field}`, which no command's compared outcomes report; "
        f"delete the entry rather than leaving a reason for a field that is gone"
        for field in sorted(set(OUTCOME_SPECIFIC) - participating)
    ] + [
        f"WITHOUT_A_CALL declares `{command}`'s `{status}` record, which no longer exists"
        for command, status in sorted(set(WITHOUT_A_CALL) - pairs)
    ]


# ------------------------------------------------------------------------------------------------
# Entry point
# ------------------------------------------------------------------------------------------------


def explain(by_command: dict[str, list[Record]], unattributed: list[Unattributed]) -> None:
    """Print what was derived, so the scope of the claim can be read rather than trusted."""
    for command in sorted(by_command):
        print(f"{command}")
        for record in by_command[command]:
            if not record.is_outcome:
                kind = "fragment"
            elif (record.command, record.status) in WITHOUT_A_CALL:
                kind = "no call yet"
            else:
                kind = "outcome"
            names = " ".join(field.name for field in record.fields) or "(none)"
            print(f"  {kind:<12} {record.label:<28} line {record.line:>4}  {names}")
    print("\nnot attributable to one record (this reader's blind spot):")
    for skipped in sorted(unattributed):
        print(f"  {skipped.command}.rs:{skipped.line:<4} {skipped.name or '(computed name)'}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify outcome parity (the gate)")
    parser.add_argument("--explain", action="store_true", help="print every derived record")
    parser.add_argument(
        "--root", type=pathlib.Path, default=ROOT, help="a tree assembled by a test"
    )
    args = parser.parse_args(argv)
    if not (args.check or args.explain):
        parser.error("one of --check or --explain is required")

    by_command, unattributed = read_commands(args.root)
    if args.explain:
        explain(by_command, unattributed)
        if not args.check:
            return 0

    problems = scope_problems(by_command)
    if not problems:
        problems = (
            parity_problems(by_command)
            + documentation_problems(by_command, args.root)
            + unused_exemptions(by_command)
        )
    if problems:
        print("Command outcomes report different fields:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    # The summary states the size of the claim *and* the size of the blind spot, in one line. A
    # check that prints only what it verified reads as though it verified everything, which is how
    # the last three silent checkers in this repository were believed for as long as they were.
    siblings = compared(by_command)
    fields = compared_fields(by_command)
    print(
        f"outcome parity: {sum(len(found) for found in siblings.values())} outcomes across "
        f"{len(siblings)} commands ({', '.join(sorted(siblings))}) carry "
        f"{len(fields) - len(OUTCOME_SPECIFIC)} of {len(fields)} compared fields uniformly; "
        f"{len(OUTCOME_SPECIFIC)} are declared outcome-specific, and {REFERENCE.name} names them "
        f"all. {len(unattributed)} field additions in {COMMANDS} are not attributable to one "
        f"outcome and are not compared (--explain)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
