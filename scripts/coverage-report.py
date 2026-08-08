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
        + ".",
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
        "## What the number still cannot see",
        "",
        "- **An inline `#[cfg(test)] mod tests` lives in `src/` and is counted.** The exclusions",
        "  above are paths, and this project keeps unit tests beside the code they test, so the",
        "  figure is flattered by however much test code sits inside a source file. Integration",
        "  tests under `/tests/` are excluded; their inline siblings cannot be, by path.",
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


def check(measurement_path: pathlib.Path = MEASUREMENT, report_path: pathlib.Path = REPORT) -> int:
    """Verify the page is what the record says, and nothing about the number itself."""
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
    if args.measure:
        return measure(args.out, MEASUREMENT, REPORT)
    if args.check:
        return check()
    return write()


if __name__ == "__main__":
    sys.exit(main())
