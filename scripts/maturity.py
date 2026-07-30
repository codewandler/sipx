#!/usr/bin/env python3
"""Generate the distance to `1.0.0-alpha`, from the sources that are already checked.

Someone asked how far sipx is from v1, and the honest first answer was that the question had no
denominator: the roadmap ran M0-M12 with a deferral list and never named 1.0, and the only `v1` in
the tree was `sipx.app.v1`, a protocol version. So predicates were written down first
(`docs/roadmap.md`), and this generates the distance to them.

**Why generated and not written.** This project has paid twice for a hand-maintained list drifting
from what it described: the gate's command list (`X-22`, which hid a red `msrv` job for five days)
and the pool-key prose (`X-24`, wrong through two changes to the type). The roadmap's own Status
block said "941 tests pass" through four releases that took the real number past 1300. A maturity
number is the *most* tempting thing to hand-maintain and the least useful when stale, because the
only decision it feeds is when to cut a release.

**What this deliberately is not.** Not a dashboard. One page, from two machine-read sources plus git,
that a release decision can be made from. If it grows a second page it has failed — the vision's
non-goals discipline applies to instrumentation too.

**The blind spot, stated because an index that hides it lies.** `status = "implemented"` means the
code exists, not that a call can reach it. `X-30` demoted three rows the day it landed for exactly
that reason, and `X-33` demoted two more; the reachability check now covers `media` and `security`
and was measured and *declined* for `transport`. Outside those layers `implemented` is unverified
against callers, so every count below inherits that limit and says so.
"""

import argparse
import collections
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs" / "rfc" / "registry.toml"
STORIES = ROOT / "docs" / "stories"
REPORT = ROOT / "docs" / "maturity.md"

BEGIN = "<!-- BEGIN maturity -->"
END = "<!-- END maturity -->"

#: Statuses that count as a story still to do. `blocked` counts as open: a story parked on a
#: dependency is distance, not progress, and calling it anything else is how a board flatters itself.
OPEN = {"ready", "backlog", "blocked", "in-progress"}

#: Layers where a role claim is checked against a caller above the implementing crate by the *path*
#: check. Kept in step with `ROLE_REACHABILITY_LAYERS` in `rfc-report.py`.
#:
#: This set is deliberately **not** widened by `X-38`. Widening it would turn on the per-row path check
#: for layers `X-33` measured and declined it for, and that check is the one whose limit started this
#: whole lineage: it is satisfied by citing a file whose relevant branch is dead. `X-38` did not build
#: a better version of it — it changed what the question means, which is why the basis below is a
#: second column rather than more rows in this one.
REACHABILITY_CHECKED = {"media", "security"}

#: What now decides reachability for every layer: the shipped application, and the gate step that
#: holds the declared surface against it. `X-38`, and alpha predicate 1.
SURFACE_CHECKER = "scripts/check-app-surface.py"
SURFACE_APPLICATION = "sipx-app"


class Predicate:
    """One alpha predicate, and how its state is arrived at.

    `computed` predicates are arithmetic over the registry and the board. `attested` ones are not
    mechanically checkable — "no known-wrong shipped path" cannot be computed, because a defect
    nobody has found leaves no trace — and are reported as such with the story that would falsify
    them. Reporting an attestation as a measurement is the failure this whole file exists to avoid.
    """

    def __init__(self, number, name, kind, blockers=(), detail=""):
        self.number = number
        self.name = name
        self.kind = kind
        self.blockers = tuple(blockers)
        self.detail = detail


#: The seven predicates from `docs/roadmap.md`, with the story that closes each. A predicate is met
#: when every story named here is `done`; that is the whole rule, and it is why the stories are named
#: rather than the state being restated.
ALPHA = (
    Predicate(
        1,
        "No claim outlives its caller, at any layer",
        "computed",
        ["X-30", "X-33", "X-37", "X-38"],
        "Computed, but the thing computed is a *definition* rather than a search. `X-38` ships an "
        f"application (`{SURFACE_APPLICATION}`) and defines the reachable-from-a-call surface as what "
        f"it uses; `{SURFACE_CHECKER}` holds every crate's `Supported` claim against that "
        "application's real dependency closure, and the gate is red when the two disagree. The three "
        "path checks before it could only find capabilities that were *mentioned* — a path is "
        "satisfied by citing a file whose relevant branch is dead — and none of them could say "
        "whether a capability was worth selecting. An application answers that by needing it or not. "
        "What this does **not** say is that every row of a layer is individually reached: the "
        "declarations it checks are per crate, so the surface is entered per crate.",
    ),
    Predicate(2, "Adversarial input and adversarial timing are both fuzzed", "computed", ["X-19", "X-31"]),
    Predicate(3, "A red gate means a defect", "computed", ["X-28", "X-29", "X-34", "X-36"]),
    Predicate(
        4,
        "No known-wrong shipped path",
        "attested",
        ["S-27", "P-7"],
        "Cannot be computed: a defect nobody has found leaves no trace in either source. What is "
        "reported is the absence of *open* stories describing one.",
    ),
    Predicate(5, "The public API says what it guarantees", "computed", ["A-8"]),
    Predicate(
        6,
        "Testable from a shell for everything the CLI exposes",
        "attested",
        [],
        "Met at filing and not re-derived here: it is a property of the CLI's test suite, which the "
        "gate runs.",
    ),
    Predicate(7, "The distance to v1 is generated, not asserted", "computed", ["X-32"]),
)


def stories():
    """Every story's frontmatter, by id."""
    found = {}
    for path in sorted(STORIES.glob("*.md")):
        if path.name in {"README.md", "_TEMPLATE.md"}:
            continue
        text = path.read_text()
        match = re.match(r"---\n(.*?)\n---", text, re.S)
        if not match:
            continue
        fields = {}
        for line in match.group(1).splitlines():
            key, _, value = line.partition(":")
            fields[key.strip()] = value.strip()
        if "id" in fields:
            found[fields["id"]] = fields
    return found


def registry():
    return tomllib.loads(REGISTRY.read_text())["rfc"]


def git_lines(args):
    """A git invocation, or `None` when git cannot answer.

    Absent git is not a failure: the report says the rate is unavailable rather than reporting zero,
    because zero filed and zero closed would read as a converged project.
    """
    try:
        done = subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True, check=False
        )
    except OSError:
        return None
    if done.returncode != 0:
        return None
    return done.stdout.splitlines()


def discovery_rate():
    """Stories filed and closed per day, from git.

    The least obvious output here and the most useful. Burn-down is not a maturity signal while
    discovery outpaces closure: a shrinking board means the authors have stopped being surprised,
    and a growing one means the opposite however much gets done. The date the crossover becomes
    *durable* is the real marker.

    Filed is a story file being added. Closed is a `status: done` line appearing — so a story that
    is reopened and closed again counts twice, which is the honest reading of "closed on that day".
    """
    filed = collections.Counter()
    lines = git_lines(
        ["log", "--date=short", "--format=C %ad", "--diff-filter=A", "--name-only", "--", "docs/stories"]
    )
    if lines is None:
        return None
    day = None
    for line in lines:
        if line.startswith("C "):
            day = line[2:].strip()
        elif line.strip().endswith(".md") and day:
            name = pathlib.PurePosixPath(line.strip()).name
            if name not in {"README.md", "_TEMPLATE.md"}:
                filed[day] += 1

    closed = collections.Counter()
    lines = git_lines(
        ["log", "--date=short", "--format=C %ad", "-p", "--unified=0", "--", "docs/stories"]
    )
    if lines is None:
        return None
    day = None
    for line in lines:
        if line.startswith("C "):
            day = line[2:].strip()
        elif line.startswith("+status: done") and day:
            closed[day] += 1
    return filed, closed


def layer_counts(rows):
    """RFC status counts per layer, and the `other` bucket made explicit.

    `other` exists so the four named columns plus it always sum to the layer's total. A table whose
    rows do not add up invites the reader to derive a percentage from the columns that are shown.
    """
    by_layer = collections.defaultdict(collections.Counter)
    for row in rows:
        by_layer[row.get("layer", "?")][row.get("status", "?")] += 1
    summary = {}
    for layer, counts in by_layer.items():
        total = sum(counts.values())
        summary[layer] = {
            "total": total,
            "implemented": counts["implemented"],
            "partial": counts["partial"],
            "none": counts["none"],
            "other": total - counts["implemented"] - counts["partial"] - counts["none"],
        }
    return summary


def pillar_counts(found):
    """Open stories per pillar, and the number done."""
    per_pillar = collections.Counter()
    done = 0
    for fields in found.values():
        if fields.get("status") in OPEN:
            per_pillar[fields.get("pillar", "?")] += 1
        elif fields.get("status") == "done":
            done += 1
    return per_pillar, done


def predicate_state(predicate, found):
    """Whether a predicate's stories are all closed, and which are not."""
    open_blockers = [
        blocker
        for blocker in predicate.blockers
        if found.get(blocker, {}).get("status", "ready") in OPEN
    ]
    missing = [blocker for blocker in predicate.blockers if blocker not in found]
    return open_blockers, missing


def render():
    found = stories()
    rows = registry()

    lines = [BEGIN, ""]

    # ---- the alpha, which is the only question this file exists to answer
    met = 0
    lines.append("## Distance to `1.0.0-alpha`")
    lines.append("")
    lines.append("| # | Predicate | State | Waiting on |")
    lines.append("|---|---|---|---|")
    for predicate in ALPHA:
        open_blockers, missing = predicate_state(predicate, found)
        if missing:
            state = "**unknown**"
            waiting = f"story {', '.join(missing)} not in the board"
        elif open_blockers:
            state = "open"
            waiting = ", ".join(f"`{blocker}`" for blocker in open_blockers)
        else:
            state = "met" if predicate.kind == "computed" else "met (attested)"
            waiting = "—"
            met += 1
        lines.append(f"| {predicate.number} | {predicate.name} | {state} | {waiting} |")
    lines.append("")
    lines.append(
        f"**{met} of {len(ALPHA)} predicates met.** A predicate is met when every story named for it "
        "is `done` — the stories are the definition, so this table cannot drift from the board."
    )
    lines.append("")
    # The label follows the kind. An earlier version printed "is attested, not computed" over every
    # note, which would have described predicate 1's mechanical check as an attestation — the exact
    # confusion this file's docstring says it exists to avoid, one level up.
    for predicate in ALPHA:
        if not predicate.detail:
            continue
        if predicate.kind == "attested":
            label = f"Predicate {predicate.number} is attested, not computed."
        else:
            label = f"Predicate {predicate.number} is computed, not attested."
        lines.append(f"- **{label}** {predicate.detail}")
    lines.append("")

    # ---- RFCs per layer. One aggregate percentage would call unlike layers alike.
    lines.append("## RFC coverage, per layer")
    lines.append("")
    lines.append(
        "No aggregate percentage is given. `media` and `core` differ in size and in how much of each "
        "is reachable, and one number would call them the same. **`partial` is counted as `partial`** "
        "and never as a fraction of done."
    )
    lines.append("")
    lines.append("| Layer | RFCs | implemented | partial | none | other | Reachability basis |")
    lines.append("|---|---|---|---|---|---|---|")
    summary = layer_counts(rows)
    for layer in sorted(summary):
        counts = summary[layer]
        # Every layer now has an application under it, which is what `X-38` changed. Two of them also
        # have the per-row path check, and that is strictly more than the others have — so the column
        # says which, rather than flattening both into "yes".
        basis = (
            "application + path check"
            if layer in REACHABILITY_CHECKED
            else "application"
        )
        lines.append(
            f"| {layer} | {counts['total']} | {counts['implemented']} | {counts['partial']} | "
            f"{counts['none']} | {counts['other']} | {basis} |"
        )
    total = len(rows)
    lines.append("")
    lines.append(
        f"{total} RFCs tracked. **`implemented` now means the code exists in a crate the shipped "
        f"application depends on** — that is the definition `X-38` put in place of the caveat this "
        f"table carried for `core`, `services`, `transport` and `wire`, and `{SURFACE_CHECKER}` fails "
        f"the gate when a crate claims supported surface no application reaches. The two layers that "
        f"also say *path check* carry `rfc-report.py`'s per-row check on top; the others are entered "
        f"per crate, so a single row of them is not individually attested."
    )
    lines.append("")

    # ---- open work, per pillar
    lines.append("## Open work, per pillar")
    lines.append("")
    per_pillar, done_total = pillar_counts(found)
    lines.append("| Pillar | Open stories |")
    lines.append("|---|---|")
    for pillar, count in sorted(per_pillar.items(), key=lambda item: (-item[1], item[0])):
        lines.append(f"| {pillar} | {count} |")
    lines.append(f"| **total** | **{sum(per_pillar.values())}** |")
    lines.append("")
    lines.append(
        f"{done_total} stories done. `blocked` counts as open: a story parked on a dependency is "
        "distance, not progress."
    )
    lines.append("")

    # ---- discovery versus closure
    lines.append("## Discovery versus closure")
    lines.append("")
    rate = discovery_rate()
    if rate is None:
        lines.append(
            "Unavailable: this is read from git history, and git could not be asked. Not reported as "
            "zero, because zero filed and zero closed would read as a converged project."
        )
    else:
        filed, closed = rate
        days = sorted(set(filed) | set(closed))[-10:]
        lines.append(
            "Burn-down is not a maturity signal while discovery outpaces closure. The marker to watch "
            "is not a single day where closure wins but the date that crossover becomes **durable** — "
            "that is when the codebase stops surprising its authors."
        )
        lines.append("")
        lines.append("| Day | Filed | Closed | Net |")
        lines.append("|---|---|---|---|")
        for day in days:
            net = closed[day] - filed[day]
            lines.append(f"| {day} | {filed[day]} | {closed[day]} | {net:+d} |")
        lines.append("")
        lines.append(
            "Filed is a story file being added; closed is a `status: done` line appearing, so a story "
            "reopened and closed again counts twice — which is the honest reading of *closed that day*."
        )
    lines.append("")

    # ---- the limits, last, because a reader who stops early should still have seen the table
    lines.append("## What this cannot see")
    lines.append("")
    lines.append(
        "- **The reachable surface is one application's opinion, and it is entered per crate.** "
        f"`X-38` replaced *unverified against callers* with a definition: what `{SURFACE_APPLICATION}` "
        "uses is the surface. That is a real caller rather than a grep, and it is also a single one — "
        "so a supported *module* that the application's crates never name is not caught, because the "
        "declarations being checked are per crate. A second application disagreeing is the intended "
        "way this widens, and the rule for it is in `README.md`: an experimental item that something "
        "outside this repository depends on graduates, with a `CHANGELOG.md` entry."
    )
    lines.append(
        "- **A predicate here is only as good as its story list.** Predicate 4 in particular reports "
        "the absence of open stories describing a known-wrong path, which is not the same as there "
        "being none. `S-27` — a `sips:` URI dialled in cleartext — was found on the day it was filed, "
        "not by this report."
    )
    lines.append(
        "- **Nothing here measures whether the tests are good**, only that they pass. Predicate 3 "
        "exists because a test can be green and assert nothing: `X-36` found one that could not detect "
        "the reversal of the invariant it was named for."
    )
    lines.append("")
    lines.append(END)
    return "\n".join(lines) + "\n"


def existing():
    if not REPORT.exists():
        return None
    text = REPORT.read_text()
    if BEGIN not in text or END not in text:
        return None
    start = text.index(BEGIN)
    end = text.index(END) + len(END)
    return text[start:end] + "\n"


def write(generated):
    if REPORT.exists():
        text = REPORT.read_text()
        if BEGIN in text and END in text:
            start = text.index(BEGIN)
            end = text.index(END) + len(END)
            REPORT.write_text(text[:start] + generated.rstrip("\n") + text[end:])
            return
    REPORT.write_text(
        "# Maturity: the distance to `1.0.0-alpha`\n\n"
        "Generated by `scripts/maturity.py` from `docs/rfc/registry.toml`, story frontmatter and git.\n"
        "**Do not edit between the markers.** Everything outside them is hand-written and preserved.\n\n"
        f"{generated}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the generated report is not what the sources say, and write nothing",
    )
    args = parser.parse_args()

    generated = render()
    if args.check:
        current = existing()
        if current is None:
            print(
                f"{REPORT.relative_to(ROOT)} is missing or has lost its markers; run "
                "./scripts/maturity.py",
                file=sys.stderr,
            )
            return 1
        if current.strip() != generated.strip():
            print(
                f"{REPORT.relative_to(ROOT)} has drifted from the registry and the board; run "
                "./scripts/maturity.py",
                file=sys.stderr,
            )
            return 1
        met = generated.count("| met")
        print(f"maturity: {len(ALPHA)} alpha predicates, report current")
        return 0

    write(generated)
    print(f"maturity: wrote {REPORT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
