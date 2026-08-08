#!/usr/bin/env python3
"""Render docs/coverage.md from a recorded `cargo llvm-cov` measurement, and never act on it.

`docs/maturity.md` already states the limit this file lives inside: *"Nothing here measures whether
the tests are good, only that they pass."* Coverage does not repair that sentence. It bounds it —
it says which lines the suite never executes, and says nothing at all about whether executing them
proved anything. `X-36` found a test that ran the code it was named for and could not detect the
reversal of its own invariant, and that test had coverage of every line it touched.

So this file publishes a number and refuses to do anything else with it.

**Never a gate.** `docs/roadmap.md` rules out a v1 gate built on coverage in as many words: a
percentage would contradict the document it is supposed to serve. No threshold appears here, no
`--fail-under` reaches the measurement command, and a measurement that covers nothing at all checks
green. A coverage ratchet rewards tests written to touch lines, which is `X-36` reached from a new
direction: it looks like coverage and is not. Measure first; decide about a ratchet later, with the
number in hand, as its own decision.

**Never transcribed.** `docs/roadmap.md`'s Status block said "941 tests pass" through four releases
in which the real number went past 1300, and a percentage is worse than a count because nobody can
sanity-check one by eye. So the record in `docs/coverage/measurement.json` holds *counts* written by
the tool — a stored percentage is rejected by the schema — every percentage on the page is arithmetic
performed at render time, and `--check` byte-compares the page against that arithmetic. Editing the
figure by hand fails the gate.

**Excluded by a rule, never by a list.** Test code is executed by definition, so counting it
measures the suite against itself. Paths handle most of it, but `--ignore-filename-regex` removes a
*file* and this project keeps its unit tests inside the file they test — so `X-116` reaches them
with `#[coverage(off)]` in the source instead. The attribute is applied by `--annotate` and checked
by `--check`, both from the same syntactic scan for `#[cfg(test)] mod`: no file is named anywhere,
because a hand-maintained list of annotated files would rot on the first new module and rot
silently, the number simply going back up with nothing failing.

**Two halves, deliberately split.** Measuring means an instrumented rebuild of the workspace and a
full run of the suite, which is minutes and a second copy of `target/`; rendering and comparing needs
nothing but this file and a JSON document. So `--measure` runs in CI, where the raw reports are
uploaded as a build artifact, and the cheap half is an ordinary gate step every implementor runs. The
recorded figure therefore describes the commit it was taken at and not necessarily `HEAD`, which the
page states rather than implies.
"""

import argparse
import datetime
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MEASUREMENT = ROOT / "docs" / "coverage" / "measurement.json"
REPORT = ROOT / "docs" / "coverage.md"

#: The flag that takes a new measurement, named once because `ci.yml` runs it and the test suite
#: asserts that the job runs *this* mode rather than some other invocation of the tool.
MEASURE_FLAG = "--measure"

#: `--branch` is unstable in `cargo llvm-cov`, so branch coverage needs a nightly compiler. The
#: `fuzz` job already establishes that a nightly toolchain is allowed for work that is measured
#: rather than shipped; nothing built here reaches a release artifact.
TOOLCHAIN = "nightly"

#: What the measurement leaves out, and why each one would otherwise make the number mean less.
#: Every entry is applied to the measurement (`--ignore-filename-regex`) *and* printed on the page,
#: because a page that lists an exclusion the tool never applied describes a measurement nobody took.
EXCLUDED = (
    (
        "/tests/",
        "test code is executed by definition, so counting it measures the suite against itself: "
        "the figure would rise for writing tests and never fall for writing untested code",
    ),
    (
        "/examples/",
        "the examples are compiled by the gate and executed by the guides' own checks, not by the "
        "suite this measures",
    ),
    (
        "/benches/",
        "benchmark harnesses are measurement scaffolding, and their own execution says nothing "
        "about the shipped code",
    ),
    (
        "/fuzz/",
        "the fuzz targets are a separate cargo workspace with its own nightly campaign; a target "
        "the suite never invokes would read as unreached code that is in fact exercised elsewhere",
    ),
    (
        "/target/",
        "generated code — build-script output is compiled from the build directory, and it is "
        "written rather than authored, so its reached fraction is not a fact about this repository",
    ),
)

#: One regex, because `--ignore-filename-regex` takes one.
IGNORE_REGEX = "|".join(pattern for pattern, _ in EXCLUDED)

#: The cfg `cargo llvm-cov` sets on the crates it instruments, and the reason the exclusion below
#: costs a stable build nothing: outside a coverage run the cfg is never set, so every `cfg_attr`
#: guarded by it is parsed and discarded.
COVERAGE_CFG = "coverage_nightly"

#: What an inline test module carries, and what a crate root carries so that it may. `#[coverage]`
#: is the unstable `coverage_attribute` feature, which is why both are `cfg_attr` rather than plain
#: attributes — the measurement already needs nightly for `--branch`, and nothing else may.
MODULE_ATTRIBUTE = f"#[cfg_attr({COVERAGE_CFG}, coverage(off))]"
CRATE_ATTRIBUTE = f"#![cfg_attr({COVERAGE_CFG}, feature(coverage_attribute))]"

#: What the crate attribute is introduced by, so that a reader who meets it in a crate root is told
#: where to go rather than left to guess why a coverage cfg is in their library.
CRATE_ATTRIBUTE_COMMENT = (
    "// This crate's inline `#[cfg(test)]` modules opt out of coverage instrumentation, so the",
    "// published figure measures the code rather than the tests measuring it. Never set outside",
    "// `cargo llvm-cov`, so every other build parses this and discards it. Applied by",
    "// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.",
)

#: The attribute the rule keys on. Recognised as a line of its own, which is how every one of them
#: is written in this workspace and what `--annotate` produces.
CFG_TEST = "#[cfg(test)]"

#: Where the rule looks. The workspace is `crates/*`, and only `src/` matters: `tests/`, `benches/`
#: and `examples/` are already removed by path, above.
SOURCES = ROOT / "crates"

#: A crate root is where a `feature` gate has to be declared, and these are cargo's own default
#: target paths — a rule rather than a list, so a new crate or a new binary is covered by existing
#: code the day it appears.
CRATE_ROOT_GLOBS = ("*/src/lib.rs", "*/src/main.rs", "*/src/bin/*.rs")

#: The flag that applies the rule. Named once, because the diagnostics quote it.
ANNOTATE_FLAG = "--annotate"

#: The exclusion no path can express, and why. Shaped like `EXCLUDED` and printed the same way:
#: an exclusion the page claims is one the measurement has to have made.
SOURCE_EXCLUDED = (
    (
        MODULE_ATTRIBUTE,
        "an inline `#[cfg(test)] mod` is test code in the middle of a source file, which no "
        "filename pattern can reach; this removes it from the instrumentation instead, so it "
        "leaves the figure for the same reason `/tests/` does",
    ),
)

#: Sentences the page must carry about the mechanism, asserted by the test suite for the reason
#: `DISCLAIMED` is: a source-level exclusion is invisible from the number, so the page states what
#: was done to the measurement and what it cost, or the number is unexplained.
MECHANISM_STATED = (
    MODULE_ATTRIBUTE,
    "unstable `coverage_attribute` feature",
    "inert in every build that is not a coverage run",
)

#: The counters read out of the tool, in the order the page prints them. `lines` and `branches` are
#: the two the story asks for; `functions` costs nothing and is the one that most often shows a
#: module nothing calls.
COUNTERS = ("lines", "branches", "functions")

#: What `covered` and `total` are called in `llvm-cov`'s own export.
LLVM_COVERED = "covered"
LLVM_TOTAL = "count"

#: Printed where a fraction has no denominator. Zero of zero is not zero percent, and rendering it as
#: `0.00%` would be a claim about code that does not exist.
NO_DATA = "—"

#: Sentences the page must carry, asserted by the test suite. They are the reason this measurement is
#: allowed to exist at all, so they are not decoration that a later edit can quietly drop.
DISCLAIMED = (
    "No threshold gates the build on any number on this page",
    "what the suite executes, not whether executing it proved anything",
)

BEGIN = "<!-- BEGIN coverage -->"
END = "<!-- END coverage -->"

#: How to refresh the figure, named in the page and in the diagnostics, because a reader who finds
#: the number stale needs the command rather than an invitation to read this file.
REFRESH_COMMAND = f"./scripts/coverage-report.py {MEASURE_FLAG}"

#: How to apply the source-level exclusion, quoted by the checker for the same reason: a module that
#: escaped the rule is fixed by re-running the rule, never by editing a file the checker named.
ANNOTATE_COMMAND = f"./scripts/coverage-report.py {ANNOTATE_FLAG}"

#: Where `--measure` writes the raw reports when no destination is given.
DEFAULT_OUT = ROOT / "target" / "coverage"

JSON_NAME = "coverage.json"
LCOV_NAME = "coverage.lcov"

#: The short form CI appends to its run summary, so the number is published somewhere a reader of
#: the run meets it rather than only inside a downloadable archive.
SUMMARY_NAME = "summary.md"


# --------------------------------------------------------------------------------------------
# The commands
# --------------------------------------------------------------------------------------------


def measure_command(out_dir: pathlib.Path | None = None) -> list[str]:
    """The invocation whose output *is* the recorded figure.

    Recorded without its destination: where the JSON was written is not part of what was measured,
    and a runner's temporary directory in a committed document would be noise that changes every
    run. Everything that decides the number — the feature set, the branch instrumentation and the
    exclusions — is in the argv.
    """
    argv = [
        "cargo",
        f"+{TOOLCHAIN}",
        "llvm-cov",
        "--workspace",
        "--all-features",
        "--branch",
        "--ignore-filename-regex",
        IGNORE_REGEX,
        "--json",
        "--summary-only",
    ]
    if out_dir is not None:
        argv += ["--output-path", str(out_dir / JSON_NAME)]
    return argv


def artifact_commands(out_dir: pathlib.Path) -> list[list[str]]:
    """Re-exports of the same profile data for a human to read: `lcov`, and a browsable tree.

    These re-render what has already been measured, so they add no build. They are the artifact
    the story asks CI to publish; the figure on the page comes from `measure_command` alone.
    """
    common = [
        "cargo",
        f"+{TOOLCHAIN}",
        "llvm-cov",
        "report",
        "--branch",
        "--ignore-filename-regex",
        IGNORE_REGEX,
    ]
    return [
        [*common, "--lcov", "--output-path", str(out_dir / LCOV_NAME)],
        # `--output-dir` is the parent: the tool creates `html/` inside whatever it is given.
        [*common, "--html", "--output-dir", str(out_dir)],
    ]


# --------------------------------------------------------------------------------------------
# The source-level exclusion
# --------------------------------------------------------------------------------------------
#
# One scan, read two ways: `--annotate` applies it and `--check` verifies it. They share a function
# rather than a convention, because the failure this guards against is a module that escapes the
# rule — and two implementations of "what a test module looks like" is exactly how one escapes.


#: `mod tests {` and `mod vectors;` both put test code under `src/`; the second only moves it to a
#: sibling file, which is still not a path any exclusion above can name.
MODULE_ITEM = re.compile(r"\s*(pub(\([\w:]+\))?\s+)?mod\s+(\w+)\s*[{;]")


def attribute_end(lines: list[str], start: int) -> int:
    """The index after the attribute beginning at `start`, which may span lines.

    `#[allow(…)]` is written across five lines throughout this workspace, so a scan that assumes one
    attribute per line stops at the first one and never reaches the `mod` behind it.
    """
    depth = 0
    index = start
    while index < len(lines):
        depth += lines[index].count("[") - lines[index].count("]")
        index += 1
        if depth <= 0:
            break
    return index


def inline_test_modules(lines: list[str]) -> list[tuple[int, str, bool]]:
    """Every `#[cfg(test)] mod` in one file: where its `#[cfg(test)]` is, its name, and whether it
    already carries the exclusion.

    Syntactic and deliberately shallow — it reads attributes and the item they sit on, and knows
    nothing about module nesting. A test module inside a test module would be annotated twice, which
    is redundant rather than wrong; the alternative is counting braces through string literals, and
    a miscount there would silently swallow the next module in the file.
    """
    found = []
    index = 0
    while index < len(lines):
        if lines[index].strip() != CFG_TEST:
            index += 1
            continue
        item = attribute_end(lines, index)
        while item < len(lines) and lines[item].lstrip().startswith("#["):
            item = attribute_end(lines, item)
        if item < len(lines) and MODULE_ITEM.match(lines[item]):
            name = MODULE_ITEM.match(lines[item]).group(3)
            annotated = any(line.strip() == MODULE_ATTRIBUTE for line in lines[index:item])
            found.append((index, name, annotated))
        index += 1
    return found


def prologue_end(lines: list[str]) -> int:
    """Where a crate root's inner attributes end, which is where another one belongs.

    After the last `//!` or `#![…]` rather than after every leading comment: a `///` doc comment or a
    plain `//` note introduces the item under it, and an attribute inserted between the two would
    separate a doc comment from what it documents.
    """
    index = 0
    last = 0
    while index < len(lines):
        stripped = lines[index].strip()
        if not stripped:
            index += 1
        elif stripped.startswith("//!"):
            index += 1
            last = index
        elif stripped.startswith("#!["):
            index = attribute_end(lines, index)
            last = index
        elif stripped.startswith("//"):
            index += 1
        else:
            break
    return last


def annotated_source(text: str, is_crate_root: bool) -> str:
    """One source file with the exclusion applied, and unchanged if it already carries it."""
    lines = text.split("\n")
    for start, _, already in reversed(inline_test_modules(lines)):
        if already:
            continue
        indent = lines[start][: len(lines[start]) - len(lines[start].lstrip())]
        lines.insert(start + 1, indent + MODULE_ATTRIBUTE)
    if is_crate_root and not any(line.strip() == CRATE_ATTRIBUTE for line in lines):
        at = prologue_end(lines)
        block = [*CRATE_ATTRIBUTE_COMMENT, CRATE_ATTRIBUTE]
        if at > 0 and lines[at - 1].strip():
            block.insert(0, "")
        if at < len(lines) and lines[at].strip():
            block.append("")
        lines[at:at] = block
    return "\n".join(lines)


def source_files(sources: pathlib.Path = SOURCES) -> list[pathlib.Path]:
    return sorted(sources.glob("*/src/**/*.rs"))


def crate_roots(sources: pathlib.Path = SOURCES) -> list[pathlib.Path]:
    return sorted({path for glob in CRATE_ROOT_GLOBS for path in sources.glob(glob)})


def named(path: pathlib.Path, sources: pathlib.Path) -> str:
    """A path a reader can open, whether it is in this repository or a fixture tree."""
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path.relative_to(sources))


def annotation_problems(sources: pathlib.Path = SOURCES) -> list[str]:
    """Every inline test module the measurement would still count, as sentences.

    Not a threshold and not a number: this reports which modules escaped the rule, never how many
    lines any of them holds. A workspace with no inline tests at all reports nothing.
    """
    escaped = []
    for path in source_files(sources):
        lines = path.read_text(encoding="utf-8").split("\n")
        escaped += [
            f"{named(path, sources)}:{start + 1} `mod {name}`"
            for start, name, annotated in inline_test_modules(lines)
            if not annotated
        ]
    bare = [
        named(path, sources)
        for path in crate_roots(sources)
        if CRATE_ATTRIBUTE not in path.read_text(encoding="utf-8")
    ]
    problems = []
    if escaped:
        problems.append(
            f"{len(escaped)} inline test modules do not carry {MODULE_ATTRIBUTE}, so the "
            f"measurement counts test code as code under test — apply the rule with "
            f"`{ANNOTATE_COMMAND}`. First: " + ", ".join(escaped[:3])
        )
    if bare:
        problems.append(
            f"{len(bare)} crate roots do not declare {CRATE_ATTRIBUTE}, so the exclusion in them "
            f"would not compile under the coverage cfg — apply the rule with `{ANNOTATE_COMMAND}`. "
            "First: " + ", ".join(bare[:3])
        )
    return problems


def annotate_sources(sources: pathlib.Path = SOURCES) -> list[pathlib.Path]:
    """Apply the rule, and report only the files it changed."""
    roots = set(crate_roots(sources))
    changed = []
    for path in sorted(set(source_files(sources)) | roots):
        text = path.read_text(encoding="utf-8")
        annotated = annotated_source(text, path in roots)
        if annotated != text:
            path.write_text(annotated, encoding="utf-8")
            changed.append(path)
    return changed


# --------------------------------------------------------------------------------------------
# The record
# --------------------------------------------------------------------------------------------


def percentage(covered: int, total: int) -> str:
    """A fraction of a whole, computed here and stored nowhere.

    Two decimals rather than one: the workspace has tens of thousands of lines, so a single decimal
    quantises hundreds of them into the same printed number and makes two genuinely different
    measurements look identical.
    """
    if total <= 0:
        return NO_DATA
    return f"{100.0 * covered / total:.2f}%"


def counters_problems(where: str, counts: object) -> list[str]:
    """Whether one group of counters is a pair of integers per counter, and nothing else."""
    if not isinstance(counts, dict):
        return [f"{where} is not an object"]
    problems = []
    for counter in COUNTERS:
        entry = counts.get(counter)
        if not isinstance(entry, dict):
            problems.append(f"{where}.{counter} is missing")
            continue
        extra = sorted(set(entry) - {"covered", "total"})
        if extra:
            # The one that matters is `percent`. `llvm-cov` exports it and it is deliberately
            # dropped on the way in: a stored percentage is a number a person could edit, and the
            # whole point of the record is that the counts are the only thing written down.
            problems.append(
                f"{where}.{counter} carries {', '.join(extra)}; only covered and total are "
                f"recorded, because a stored percent is a figure that can be typed"
            )
        for field in ("covered", "total"):
            value = entry.get(field)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                problems.append(f"{where}.{counter}.{field} is not a count")
        covered, total = entry.get("covered"), entry.get("total")
        if isinstance(covered, int) and isinstance(total, int) and covered > total:
            problems.append(f"{where}.{counter} covers {covered} of {total}")
    return problems


def schema_problems(data: object) -> list[str]:
    """Everything wrong with a measurement record, as sentences rather than a traceback.

    A malformed record reaches `render` otherwise, and a `KeyError` out of a renderer tells whoever
    broke it nothing about what to do next.
    """
    if not isinstance(data, dict):
        return ["the measurement is not an object"]
    problems = []
    for field, kind in (
        ("tool", str),
        ("tool_version", str),
        ("toolchain", str),
        ("measured_at", str),
        ("commit", str),
    ):
        if not isinstance(data.get(field), kind) or not data.get(field):
            problems.append(f"{field} is missing")
    commit = data.get("commit")
    if isinstance(commit, str) and commit and not re.fullmatch(r"[0-9a-f]{40}", commit):
        problems.append("commit is not a full git object name")
    measured_at = data.get("measured_at")
    if isinstance(measured_at, str) and measured_at:
        try:
            datetime.date.fromisoformat(measured_at)
        except ValueError:
            problems.append(f"measured_at {measured_at!r} is not an ISO YYYY-MM-DD date")
    command = data.get("command")
    if not isinstance(command, list) or not all(isinstance(word, str) for word in command):
        problems.append("command is not a list of words")

    excluded = data.get("excluded")
    if not isinstance(excluded, list):
        problems.append("excluded is missing")
    else:
        recorded = [
            (entry.get("pattern"), entry.get("why"))
            for entry in excluded
            if isinstance(entry, dict)
        ]
        if recorded != [(pattern, why) for pattern, why in EXCLUDED]:
            problems.append(
                "excluded does not match the exclusions this script applies; the page would list "
                f"exclusions the measurement never made — re-measure with `{REFRESH_COMMAND}`"
            )

    source_excluded = data.get("source_excluded")
    if not isinstance(source_excluded, list):
        problems.append("source_excluded is missing")
    else:
        recorded = [
            (entry.get("attribute"), entry.get("why"))
            for entry in source_excluded
            if isinstance(entry, dict)
        ]
        if recorded != [(attribute, why) for attribute, why in SOURCE_EXCLUDED]:
            problems.append(
                "source_excluded does not match the source-level exclusion this script applies; "
                f"the page would claim an exclusion the build never made — re-measure with "
                f"`{REFRESH_COMMAND}`"
            )
    modules = data.get("test_modules_excluded")
    if not isinstance(modules, int) or isinstance(modules, bool) or modules < 0:
        # A count, like every other figure in this record, and for the same reason: the page states
        # how many test modules left the measurement, and a reader can check it with one `grep`.
        problems.append("test_modules_excluded is not a count")

    counter_problems = counters_problems("totals", data.get("totals"))
    crates = data.get("crates")
    if not isinstance(crates, dict) or not crates:
        counter_problems.append("crates is missing")
    else:
        for name in sorted(crates):
            counter_problems += counters_problems(f"crates.{name}", crates[name])
    problems += counter_problems
    if not counter_problems:
        problems += arithmetic_problems(data["totals"], crates)
    return problems


def arithmetic_problems(totals: dict, crates: dict) -> list[str]:
    """Whether the per-crate rows still sum to the workspace row.

    The two tables on the page are the same files counted twice, so they add up or one of them is
    wrong. Checked rather than assumed, because this is the shape a hand-edited record takes: the
    page is rendered from the record, so editing the record moves the page and the byte-compare
    notices nothing. `maturity.py` makes the same argument for its `other` bucket — a table whose
    rows do not add up invites the reader to derive a number nobody measured.
    """
    problems = []
    for counter in COUNTERS:
        for field in ("covered", "total"):
            summed = sum(counts[counter][field] for counts in crates.values())
            if summed != totals[counter][field]:
                problems.append(
                    f"the crates sum to {summed} {counter} {field} and totals says "
                    f"{totals[counter][field]}; the published tables would not add up"
                )
    return problems


def read_counts(block: dict) -> dict:
    """One `llvm-cov` summary block, reduced to the counts and stripped of its percentages."""
    return {
        counter: {
            "covered": int(block[counter][LLVM_COVERED]),
            "total": int(block[counter][LLVM_TOTAL]),
        }
        for counter in COUNTERS
    }


def add_counts(into: dict, block: dict) -> dict:
    """Accumulate one file's counts into a crate's."""
    for counter in COUNTERS:
        into.setdefault(counter, {"covered": 0, "total": 0})
        into[counter]["covered"] += int(block[counter][LLVM_COVERED])
        into[counter]["total"] += int(block[counter][LLVM_TOTAL])
    return into


def crate_of(filename: str) -> str | None:
    """Which workspace crate a measured file belongs to, or `None` for a file outside the workspace.

    The workspace is `crates/*`, so the segment after `crates/` is the crate. `None` is not a bucket
    to fold into a neighbour: a file the exclusions did not remove and no crate owns would make the
    per-crate table stop summing to the workspace row, and a table whose rows do not add up invites
    the reader to derive a number nobody measured. `measurement_from_export` refuses instead.
    """
    parts = pathlib.PurePosixPath(filename).parts
    if "crates" in parts:
        index = parts.index("crates")
        if index + 1 < len(parts):
            return parts[index + 1]
    return None


def measurement_from_export(
    export: dict, commit: str, versions: dict
) -> tuple[dict | None, list[str]]:
    """A record, built from `llvm-cov`'s own JSON export and nothing typed.

    The workspace row is the tool's own `totals`, and the per-crate rows are this script's grouping
    of the same files. They are checked against each other rather than assumed equal, because the
    only way they can differ is a measured file no crate owns — and that is exactly the case where
    the published table would quietly stop adding up.
    """
    data = export["data"][0]
    crates: dict[str, dict] = {}
    unowned: list[str] = []
    for entry in data["files"]:
        name = crate_of(entry["filename"])
        if name is None:
            unowned.append(entry["filename"])
            continue
        add_counts(crates.setdefault(name, {}), entry["summary"])
    if unowned:
        return None, [
            f"{len(unowned)} measured files belong to no workspace crate, so the per-crate table "
            f"would not sum to the workspace row — exclude them or widen the grouping. First: "
            + ", ".join(sorted(unowned)[:3])
        ]
    return {
        "tool": "cargo-llvm-cov",
        "tool_version": versions["tool"],
        "toolchain": versions["toolchain"],
        "measured_at": datetime.date.today().isoformat(),
        "commit": commit,
        "command": measure_command(),
        "excluded": [{"pattern": pattern, "why": why} for pattern, why in EXCLUDED],
        "source_excluded": [
            {"attribute": attribute, "why": why} for attribute, why in SOURCE_EXCLUDED
        ],
        "test_modules_excluded": sum(
            len(inline_test_modules(path.read_text(encoding="utf-8").split("\n")))
            for path in source_files()
        ),
        "totals": read_counts(data["totals"]),
        "crates": {name: crates[name] for name in sorted(crates)},
    }, []


# --------------------------------------------------------------------------------------------
# The page
# --------------------------------------------------------------------------------------------


def counter_row(label: str, counts: dict) -> str:
    entry = counts[label]
    return (
        f"| {label.capitalize()} | {entry['covered']} | {entry['total']} | "
        f"{percentage(entry['covered'], entry['total'])} |"
    )


def crate_row(name: str, counts: dict) -> str:
    cells = " | ".join(
        percentage(counts[counter]["covered"], counts[counter]["total"]) for counter in COUNTERS
    )
    reached = counts["lines"]["covered"]
    total = counts["lines"]["total"]
    return f"| `{name}` | {cells} | {total - reached} |"


def summary(data: dict) -> str:
    """The short form CI pastes into its run summary, so the number is met without a download.

    Built from the same helpers as the page rather than from a second set of strings: it is a
    shorter view of the record, never a second copy of the figures.
    """
    lines = [
        "## Coverage",
        "",
        "| Counter | Covered | Total | Reached |",
        "|---|---|---|---|",
    ]
    lines += [counter_row(counter, data["totals"]) for counter in COUNTERS]
    lines += [
        "",
        f"Measured at `{data['commit']}` with `{data['tool_version']}`, excluding "
        + ", ".join(f"`{pattern}`" for pattern, _ in EXCLUDED)
        + f", and the {data['test_modules_excluded']} inline `#[cfg(test)]` modules that "
        "`#[coverage(off)]` removes from the instrumentation.",
        "",
        "**Nothing gates on this.** No threshold, no ratchet, and no failure when it drops. It "
        "measures what the suite executes, not whether executing it proved anything — see "
        "`docs/coverage.md`, which this run also re-rendered, for the per-crate figures and the "
        "limits.",
        "",
    ]
    return "\n".join(lines)


def render(data: dict) -> str:
    """The whole page. Every number in it is arithmetic over the record."""
    lines = [
        "# Coverage: what the suite executes",
        "",
        f"Generated by `scripts/coverage-report.py` from `{MEASUREMENT.relative_to(ROOT)}`, which is",
        "written by `cargo llvm-cov`. **Do not edit — the figures are arithmetic over recorded",
        "counts, and a hand-edited one fails the gate.**",
        "",
        BEGIN,
        "",
        "## What this is not",
        "",
        "**No threshold gates the build on any number on this page.** Nothing asserts one, nothing",
        "ratchets one, and a drop fails nothing — [`roadmap.md`](roadmap.md) rules out a v1 gate",
        "built on coverage, because a percentage would contradict the document it is supposed to",
        "serve. A coverage gate rewards tests written to touch lines, which is the `X-36` failure",
        "shape in a new place: it looks like coverage and is not.",
        "",
        "This measures **what the suite executes, not whether executing it proved anything**.",
        "[`maturity.md`](maturity.md) already says the harder half — *nothing there measures whether",
        "the tests are good, only that they pass* — and this page does not repair that sentence. It",
        "bounds it, by naming the code no test reaches at all.",
        "",
        "## The workspace",
        "",
        "| Counter | Covered | Total | Reached |",
        "|---|---|---|---|",
    ]
    lines += [counter_row(counter, data["totals"]) for counter in COUNTERS]
    lines += [
        "",
        f"Measured on {data['measured_at']} at commit `{data['commit']}`, with",
        f"`{data['tool_version']}` on `{data['toolchain']}`.",
        "**The figure describes that commit and not necessarily `HEAD`**: measuring costs an",
        "instrumented rebuild of the workspace and a full run of the suite, so it is taken",
        f"deliberately rather than on every push. Refresh it with `{REFRESH_COMMAND}`.",
        "",
        "The counts above come from:",
        "",
        "```sh",
        " ".join(data["command"]),
        "```",
        "",
        "## Per crate",
        "",
        "Per crate rather than one workspace number alone, for the reason",
        "[`maturity.md`](maturity.md) gives no aggregate RFC percentage: the crates differ in size",
        "and in how much of each a call can reach, and one number calls them the same. The last",
        "column is the count that actually answers *what does this suite not reach*.",
        "",
        "| Crate | Lines | Branches | Functions | Lines unreached |",
        "|---|---|---|---|---|",
    ]
    lines += [crate_row(name, data["crates"][name]) for name in sorted(data["crates"])]
    lines += [
        "",
        "## What the measurement excludes",
        "",
        "Applied to the measurement itself, not subtracted afterwards — each pattern below is passed",
        "to `--ignore-filename-regex`, and the same list is what this page prints, so a page cannot",
        "claim an exclusion the tool never made.",
        "",
        "| Path | Why |",
        "|---|---|",
    ]
    lines += [f"| `{pattern}` | {why} |" for pattern, why in EXCLUDED]
    lines += [
        "",
        "### The tests inside the source files",
        "",
        "A path pattern removes a *file*, and this project keeps its unit tests in the middle of the",
        "file they test — so the exclusions above reach `tests/` and cannot reach a",
        "`#[cfg(test)] mod tests` in `src/`. `X-66` published a figure that partly measured the",
        "tests themselves and said so; this is what closed it (`X-116`):",
        "",
        "| Attribute | Why |",
        "|---|---|",
    ]
    lines += [f"| `{attribute}` | {why} |" for attribute, why in SOURCE_EXCLUDED]
    lines += [
        "",
        f"Every `#[cfg(test)] mod` under `crates/*/src/` carries it — "
        f"{data['test_modules_excluded']} of them at the commit above — and **no file is named**",
        "anywhere. The rule is one syntactic scan, applied by",
        f"`{ANNOTATE_COMMAND}` and verified by `--check`, so a test module added tomorrow either",
        "carries the exclusion or fails an implementor's gate. A list of annotated files would rot",
        "on the first new module, and rot invisibly: the number would simply go back up.",
        "",
        "**What it cost.** `#[coverage(off)]` is the unstable `coverage_attribute` feature, so each",
        f"crate root declares `{CRATE_ATTRIBUTE}` and every",
        f"annotation is a `cfg_attr` on `{COVERAGE_CFG}` — the cfg `cargo llvm-cov` sets on what it",
        "instruments. It is therefore **inert in every build that is not a coverage run**: the",
        "stable build, the MSRV build and every release artifact parse the attribute and discard it,",
        "and the workspace declares the cfg under `[workspace.lints.rust]` so that the builds which",
        "never set it do not warn about it either. No toolchain was added — `--branch` already",
        "required nightly — and nothing that ships changed. What it buys is that the figure below",
        "no longer rises for writing a unit test.",
        "",
        "## What the number still cannot see",
        "",
        "- **`#[cfg(test)]` on anything that is not a module is still counted.** The rule reaches a",
        "  `#[cfg(test)] mod`, which is where this workspace puts its unit tests and their fixtures.",
        "  A bare `#[cfg(test)] fn` or `#[cfg(test)] impl` beside the code it helps is not a module,",
        "  and stays in the figure.",
        "- **The exclusion holds only while the cfg is set.** Measuring with `--no-cfg-coverage`, or",
        "  with a tool that does not set it, silently restores the flattered number rather than",
        f"  failing — which is why `{REFRESH_COMMAND}` is the only recorded way to take one.",
        "- **Doctests are not instrumented, and the gate runs them.** The tool measures the `--tests`",
        "  targets, so a line whose only caller is an example in a doc comment reads here as",
        "  unreached while `cargo test --workspace --all-features` executes it. The gap is in this",
        "  direction only: nothing reads as reached that was not.",
        "- **A reached line is not a checked line.** Execution is all this instruments. `X-36` is a",
        "  test that executed its subject and could not detect the reversal of the invariant it was",
        "  named for, and no coverage tool would have said anything about it.",
        "- **Branch coverage is unstable in the tool and needs a nightly compiler.** It is measured",
        "  here for the same reason the fuzz campaign runs on nightly — the result is read, not",
        "  shipped — but it is a less settled number than the line count beside it.",
        "- **Feature-gated code is measured with every feature on.** A configuration a user actually",
        "  builds may execute less of it than this says, which `scripts/check-features.sh` compiles",
        "  and nothing here measures.",
        "",
        END,
        "",
    ]
    return "\n".join(lines)


# --------------------------------------------------------------------------------------------
# Reading, writing, checking
# --------------------------------------------------------------------------------------------


def load(path: pathlib.Path) -> tuple[dict | None, list[str]]:
    """A measurement record and everything wrong with it."""
    if not path.exists():
        return None, [
            f"{path} does not exist; no coverage has been recorded — take one with "
            f"`{REFRESH_COMMAND}`"
        ]
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        return None, [f"{path} is not readable JSON: {error}"]
    problems = schema_problems(data)
    return (None if problems else data), problems


def record(data: dict, out_dir: pathlib.Path, measurement_path: pathlib.Path) -> None:
    """Write everything a measurement leaves behind: the record, and the short form CI publishes.

    Separated from `measure` so it is reachable without a cargo run — the run summary is a file a
    CI step reads by name, and a step that fails because nothing wrote it is a defect nobody would
    meet until a push.
    """
    measurement_path.parent.mkdir(parents=True, exist_ok=True)
    measurement_path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / SUMMARY_NAME).write_text(summary(data), encoding="utf-8")


def write(measurement_path: pathlib.Path = MEASUREMENT, report_path: pathlib.Path = REPORT) -> int:
    data, problems = load(measurement_path)
    if data is None:
        report_problems(problems)
        return 1
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(render(data), encoding="utf-8")
    totals = data["totals"]["lines"]
    print(
        f"coverage: wrote {report_path} — {percentage(totals['covered'], totals['total'])} of "
        f"{totals['total']} lines reached, measured {data['measured_at']}"
    )
    return 0


def check(
    measurement_path: pathlib.Path = MEASUREMENT,
    report_path: pathlib.Path = REPORT,
    sources: pathlib.Path = SOURCES,
) -> int:
    """Verify the page is what the record says, and nothing about the number itself.

    The source scan is here rather than in `--measure` alone because that is the half nobody runs
    locally. A test module written today would otherwise re-enter the measurement at the next CI
    run, months after the diff that added it — this fails in the implementor's own gate instead.
    """
    data, problems = load(measurement_path)
    if data is None:
        report_problems(problems)
        return 1
    if not report_path.exists():
        report_problems([f"{report_path} does not exist; run ./scripts/coverage-report.py"])
        return 1
    if report_path.read_text(encoding="utf-8") != render(data):
        report_problems(
            [
                f"{report_path} is not what {measurement_path.name} says; run "
                f"./scripts/coverage-report.py. The figures are generated — do not edit them."
            ]
        )
        return 1
    escaped = annotation_problems(sources)
    if escaped:
        report_problems(escaped)
        return 1
    totals = data["totals"]
    print(
        "coverage: report current — "
        + ", ".join(
            f"{counter} {percentage(totals[counter]['covered'], totals[counter]['total'])}"
            for counter in COUNTERS
        )
        + f", measured {data['measured_at']} at {data['commit'][:12]}. Nothing gates on it."
    )
    return 0


def report_problems(problems: list[str]) -> None:
    print("The published coverage figure does not match its measurement:", file=sys.stderr)
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)


# --------------------------------------------------------------------------------------------
# Measuring
# --------------------------------------------------------------------------------------------


def run(argv: list[str]) -> int:
    print(f"==> {' '.join(argv)}", flush=True)
    return subprocess.run(argv, cwd=ROOT, check=False).returncode


def tool_versions() -> dict:
    def first_line(argv):
        done = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True, check=False)
        return done.stdout.strip().splitlines()[0] if done.returncode == 0 else ""

    return {
        "tool": first_line(["cargo", "llvm-cov", "--version"]),
        "toolchain": first_line(["rustc", f"+{TOOLCHAIN}", "--version"]),
    }


def head_commit() -> str:
    done = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=False
    )
    return done.stdout.strip() if done.returncode == 0 else ""


def measure(out_dir: pathlib.Path, measurement_path: pathlib.Path, report_path: pathlib.Path) -> int:
    """Take a measurement, record its counts, and re-render the page from them."""
    escaped = annotation_problems()
    if escaped:
        # Before the build rather than after it. A measurement taken over a tree where a test module
        # escaped the rule is a number the page would describe as excluding it, and half an hour of
        # instrumented build is a long way to go to publish that.
        report_problems(escaped)
        return 1
    out_dir.mkdir(parents=True, exist_ok=True)
    code = run(measure_command(out_dir))
    if code != 0:
        print(
            f"coverage: the measurement did not complete (exit {code}); nothing was recorded",
            file=sys.stderr,
        )
        return code
    versions = tool_versions()
    commit = head_commit()
    if not commit:
        print("coverage: git could not name HEAD, so the measurement has no subject", file=sys.stderr)
        return 1
    export = json.loads((out_dir / JSON_NAME).read_text(encoding="utf-8"))
    data, problems = measurement_from_export(export, commit, versions)
    problems += schema_problems(data) if data is not None else []
    if problems:
        report_problems(problems)
        return 1
    record(data, out_dir, measurement_path)

    # The re-exports are the artifact a reader browses. They re-render profile data that already
    # exists, so a failure here costs the artifact and not the figure — which is already recorded.
    for argv in artifact_commands(out_dir):
        if run(argv) != 0:
            print(
                "coverage: an additional report format could not be re-exported; the recorded "
                "figure above is unaffected",
                file=sys.stderr,
            )
    return write(measurement_path, report_path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify docs/coverage.md is what the recorded measurement says, and write nothing",
    )
    parser.add_argument(
        MEASURE_FLAG,
        action="store_true",
        help=(
            "run the instrumented suite, record its counts and re-render the page; needs the "
            f"{TOOLCHAIN} toolchain and cargo-llvm-cov"
        ),
    )
    parser.add_argument(
        ANNOTATE_FLAG,
        action="store_true",
        help=(
            "apply the source-level exclusion — put "
            f"{MODULE_ATTRIBUTE} on every inline `#[cfg(test)] mod` under crates/*/src/ and "
            f"{CRATE_ATTRIBUTE} in every crate root"
        ),
    )
    parser.add_argument(
        "--out",
        type=pathlib.Path,
        default=DEFAULT_OUT,
        help=f"where {MEASURE_FLAG} writes the raw reports CI publishes (default: {DEFAULT_OUT})",
    )
    args = parser.parse_args()

    if args.check and args.measure:
        print(
            f"coverage: {MEASURE_FLAG} rewrites the figure and --check verifies it; run them "
            "separately",
            file=sys.stderr,
        )
        return 1
    if args.annotate:
        changed = annotate_sources()
        for path in changed:
            print(f"coverage: excluded the inline tests in {path.relative_to(ROOT)}")
        print(
            f"coverage: {len(changed)} files annotated. The figure is unchanged until "
            f"`{REFRESH_COMMAND}` is run."
        )
        return 0
    if args.measure:
        return measure(args.out, MEASUREMENT, REPORT)
    if args.check:
        return check()
    return write()


if __name__ == "__main__":
    sys.exit(main())
