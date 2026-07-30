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
import datetime
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

#: Where `status` is defined. There is exactly one definition of each status word, it lives in this
#: table, and `rfc-report.py` is what enforces it.
RFC_README = ROOT / "docs" / "rfc" / "README.md"


def status_definition(word: str) -> str:
    """The definition of a `status` word, read from `docs/rfc/README.md`'s schema table.

    **Read rather than restated, because restating it produced a false sentence** (`X-38` rework). This
    report claimed "`implemented` now means the code exists in a crate the shipped application depends
    on", which was wrong twice over: RFC 8996 is `implemented` on the evidence of `docs/specs/sip-tls.md`
    and no crate at all, and the sentence handed the load-bearing word a second definition conflicting
    with the schema table — two meanings across the two documents a reader consults, which is precisely
    the drift this repository keeps closing. `X-38` did not change what `implemented` means. It changed
    what decides *reachability*, which is a different column.
    """
    for line in RFC_README.read_text(encoding="utf-8").splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) == 2 and cells[0] == f"`{word}`":
            return cells[1].rstrip(".")
    raise SystemExit(f"maturity: no definition of `{word}` in {RFC_README.relative_to(ROOT)}")


class Predicate:
    """One alpha predicate, and how its state is arrived at.

    `computed` predicates are arithmetic over the registry and the board. `attested` ones are not
    mechanically checkable — "no known-wrong shipped path" cannot be computed, because a defect
    nobody has found leaves no trace — and are reported as such with the story that would falsify
    them. Reporting an attestation as a measurement is the failure this whole file exists to avoid.

    **A predicate does not carry its stories.** It used to, as a literal list per predicate, and that
    list was the defect `X-42` was filed over: three defects were filed against predicate 3 in one
    session and none was added to it, so the predicate read `met` while all three were open. The
    stories declare the predicate now (`PREDICATE_FIELD`), which is the file the filer is already
    writing.
    """

    def __init__(self, number, name, kind, detail=""):
        self.number = number
        self.name = name
        self.kind = kind
        self.detail = detail


#: The frontmatter field a story uses to say which alpha predicate it bears on: `predicate: 3`, or
#: `predicate: [3, 7]` for one that bears on two. **This is the only place the association is
#: recorded.** A number no predicate carries is an error rather than a silently dropped line, because
#: a typo that reported as progress is the whole failure mode here.
PREDICATE_FIELD = "predicate"

#: The seven predicates from `docs/roadmap.md`. A predicate is met when every story declaring it is
#: `done`; that is the whole rule. What is written here is the predicate — its number, its name, and
#: whether it can be computed at all — never which stories bear on it.
ALPHA = (
    Predicate(
        1,
        "No claim outlives its caller, at any layer",
        "computed",
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
    Predicate(2, "Adversarial input and adversarial timing are both fuzzed", "computed"),
    Predicate(3, "A red gate means a defect", "computed"),
    Predicate(
        4,
        "No known-wrong shipped path",
        "attested",
        "Cannot be computed: a defect nobody has found leaves no trace in either source. What is "
        "reported is the absence of *open* stories describing one.",
    ),
    Predicate(5, "The public API says what it guarantees", "computed"),
    Predicate(
        6,
        "Testable from a shell for everything the CLI exposes",
        "attested",
        "Met at filing and not re-derived here: it is a property of the CLI's test suite, which the "
        "gate runs. No story declares it, and an attestation nothing contradicts is the one case "
        "where that is not a gap.",
    ),
    Predicate(7, "The distance to v1 is generated, not asserted", "computed"),
)


#: Files under `docs/stories` that are not stories however they are shaped. `_TEMPLATE.md` carries a
#: frontmatter `id:` of its own, so the frontmatter test below does *not* subsume this list — both
#: halves are load-bearing, which is why they live together in one function.
NOT_STORIES = {"README.md", "_TEMPLATE.md"}


def story_fields(path):
    """One story's frontmatter, or `None` when the file is not a board story.

    **The single definition of "is a story"**, because there were briefly two. `discovery_rate` used
    to decide it from the file name alone, which made a scratch `notes.md` in `docs/stories` count as
    a story filed today — a red gate for no defect, this story's own failure mode reached from a new
    direction. A name is not enough and neither is frontmatter alone: the board's template has an
    `id:` too. A story is a file this list does not name, carrying a frontmatter block with an `id`.
    """
    if path.name in NOT_STORIES:
        return None
    match = re.match(r"---\n(.*?)\n---", path.read_text(encoding="utf-8"), re.S)
    if not match:
        return None
    fields = {}
    for line in match.group(1).splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()
    return fields if "id" in fields else None


def stories():
    """Every story's frontmatter, by id."""
    found = {}
    for path in sorted(STORIES.glob("*.md")):
        fields = story_fields(path)
        if fields is not None:
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


#: The frontmatter line that closes a story. What is counted is the *event* of a story closing, and
#: the event is this line appearing — so it is matched as a line rather than parsed as frontmatter.
CLOSING_LINE = "status: done"


def closes_a_story(line):
    """Whether one line of a story file closes it.

    **One reader for this line, not two.** The two halves of the union used to disagree: history
    matched `startswith`, the working tree matched equality, and a closing line with a trailing space
    was therefore counted by history and not by the working tree — so the row moved under its own
    commit and `X-39`'s flap survived on malformed frontmatter. `M-31` is the same shape and the same
    fix: one reader decides, and both callers ask it.

    Trailing whitespace is tolerated because the frontmatter parser tolerates it — `story_fields`
    strips values, so a story whose line reads `status: done ` *is* closed on the board, and a day row
    that disagreed with the board about that would be wrong rather than merely inconsistent. Leading
    whitespace is not tolerated: frontmatter keys sit at column zero, and an indented line is inside
    something else.
    """
    return line.rstrip() == CLOSING_LINE


def uncommitted_story_facts():
    """Filed and closed counts that exist in the working tree and not yet in any commit.

    **Why the working tree is a source at all** (`X-39`). `Filed` and `Closed` come from git history,
    so the count the report must contain for the current day is created *by the commit that contains
    the report*: regenerate then commit and the report is one short, commit then regenerate and the
    report is uncommitted. No ordering satisfies it, and the gate's `maturity` step was therefore red
    in every commit that filed or closed a story — most commits — and never for a defect. It was
    regenerated twice on 2026-07-30 with nothing wrong either time.

    **The fix is the third of the three options `X-39` lists**: the day rows come from a source that
    does not move under the commit that writes them, rather than the check tolerating the in-flight
    day or the report marking it provisional. History *union* the working tree is that source —
    `git commit` only relocates a fact from the second half to the first, leaving the union alone —
    and it was chosen because the other two only move the flap. A tolerated day row is unchecked
    while it is today and strictly checked tomorrow, so it goes red on some later commit that
    touched nothing; a provisional row has the same problem or stops carrying numbers at all, and
    the crossover date is the number to watch.

    A clean tree contributes nothing, so CI and every commit that touches no story see exactly the
    history-only answer they saw before, and `--check` stays as strict as it was.

    `None` when git cannot answer, matching `discovery_rate`: an unavailable rate is reported as
    unavailable and never as zero.
    """
    added = git_lines(["diff", "HEAD", "--diff-filter=A", "--name-only", "--", "docs/stories"])
    untracked = git_lines(["ls-files", "--others", "--exclude-standard", "--", "docs/stories"])
    changed = git_lines(["diff", "HEAD", "--unified=0", "--", "docs/stories"])
    if added is None or untracked is None or changed is None:
        return None

    def is_story(line):
        """A new file in the working tree that the board would read as a story.

        Decided by `story_fields`, the same test `stories()` applies, because the file is on disk here
        and can simply be read. A name-only test counted a scratch `notes.md` as a story filed today,
        which made `--check` red on a correct tree and — worse — green in the tree holding the scratch
        file and red on a clean checkout of the same commit, since the file is never committed. Local
        green with CI red is the `X-22` failure class, so a name is not enough.

        An undecodable file is not a story rather than a crash: a stray binary somebody left in the
        directory is not this script's business, whereas a *tracked* story that cannot be read is, and
        `stories()` still fails loudly on that.
        """
        path = ROOT / line.strip()
        if path.suffix != ".md" or not path.is_file():
            return False
        try:
            return story_fields(path) is not None
        except (OSError, UnicodeDecodeError):
            return False

    untracked_stories = [line.strip() for line in untracked if is_story(line)]
    filed = len([line for line in added if is_story(line)]) + len(untracked_stories)

    # Tracked edits are read from the diff, which is what `git log -p` will show once committed.
    closed = len([line for line in changed if line.startswith("+") and closes_a_story(line[1:])])
    # An untracked file is absent from that diff, and committing it shows its whole body as `+`
    # lines — so a story filed already closed has to be counted from its content or the two halves
    # of the union would disagree about it.
    for name in untracked_stories:
        lines = (ROOT / name).read_text(encoding="utf-8").splitlines()
        closed += len([line for line in lines if closes_a_story(line)])
    return filed, closed


def discovery_rate():
    """Stories filed and closed per day, from git and the working tree.

    The least obvious output here and the most useful. Burn-down is not a maturity signal while
    discovery outpaces closure: a shrinking board means the authors have stopped being surprised,
    and a growing one means the opposite however much gets done. The date the crossover becomes
    *durable* is the real marker.

    Filed is a story file being added. Closed is a `status: done` line appearing — so a story that
    is reopened and closed again counts twice, which is the honest reading of "closed on that day".

    Committed history gives every past day. Today's row adds what the working tree holds and no
    commit does yet, so that the row does not change when that tree is committed — see
    `uncommitted_story_facts`, and `X-39` for the gate step this repaired.
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
            if name not in NOT_STORIES:
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
        elif line.startswith("+") and closes_a_story(line[1:]) and day:
            closed[day] += 1

    pending = uncommitted_story_facts()
    if pending is None:
        return None
    pending_filed, pending_closed = pending
    today = datetime.date.today().isoformat()
    # Guarded, because `Counter[key] += 0` creates the key: an unconditional bump would print a
    # 0/0 row for today on every clean tree and give the table a day that nothing happened on.
    if pending_filed:
        filed[today] += pending_filed
    if pending_closed:
        closed[today] += pending_closed
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


def story_key(story_id):
    """Board order for a story id: by prefix, then numerically.

    Lexical order would print `X-19` after `X-9`, which is not how anyone reads a board.
    """
    prefix, _, number = story_id.partition("-")
    return (prefix, int(number) if number.isdigit() else 0, story_id)


def story_predicates(story_id, fields):
    """The alpha predicates one story declares, from its `predicate:` frontmatter field.

    Accepts `3` and `[3, 7]`, because a defect can falsify two predicates and forcing a filer to pick
    one would leave the other reading `met` — the failure this field exists to remove. An empty field
    is a story that declares nothing, which is most of them, and a trailing `#` comment is ignored.

    A field that is not a list of numbers exits with a message rather than raising: a malformed
    frontmatter line is a thing to fix, not a traceback to read.
    """
    raw = fields.get(PREDICATE_FIELD, "").partition("#")[0].strip()
    if not raw:
        return ()
    numbers = []
    for token in raw.strip("[]").split(","):
        token = token.strip()
        if not token:
            continue
        if not token.isdigit():
            raise SystemExit(
                f"maturity: {story_id} has `{PREDICATE_FIELD}: {raw}`, and `{token}` is not a "
                f"predicate number; write `{PREDICATE_FIELD}: 3` or `{PREDICATE_FIELD}: [3, 7]`"
            )
        numbers.append(int(token))
    return tuple(numbers)


def predicate_stories(found):
    """Every story declaring each predicate, by predicate number.

    **The single source of the association.** Nothing else in this file may hold a list of which
    stories bear on which predicate; if it did, the two would drift and the drift is `X-42`.
    """
    known = {predicate.number for predicate in ALPHA}
    declared = collections.defaultdict(list)
    for story_id in sorted(found, key=story_key):
        for number in story_predicates(story_id, found[story_id]):
            if number not in known:
                raise SystemExit(
                    f"maturity: {story_id} declares `{PREDICATE_FIELD}: {number}`, and there is no "
                    f"alpha predicate {number} — `docs/roadmap.md` has "
                    f"{', '.join(str(item) for item in sorted(known))}"
                )
            declared[number].append(story_id)
    return declared


def predicate_state(predicate, found):
    """Which of a predicate's stories are still open, and every story that declares it.

    Both halves are returned because the report needs to tell two different things apart: a predicate
    whose stories are all closed, and a predicate no story claims at all. Calling the second one met
    would mean deleting the last story that named a predicate looked like finishing it.
    """
    declared = predicate_stories(found).get(predicate.number, [])
    open_stories = [
        story_id for story_id in declared if found[story_id].get("status", "ready") in OPEN
    ]
    return open_stories, declared


def predicate_row(predicate, found):
    """The `State` and `Waiting on` cells for one predicate."""
    open_stories, declared = predicate_state(predicate, found)
    if predicate.kind == "computed" and not declared:
        # Not met: unrecorded. A computed predicate is computed over stories, and there are none to
        # compute over — so the honest report is that nobody has said what would close it.
        return "**unknown**", f"no story declares `{PREDICATE_FIELD}: {predicate.number}`"
    if open_stories:
        return "open", ", ".join(f"`{story_id}`" for story_id in open_stories)
    return ("met" if predicate.kind == "computed" else "met (attested)"), "—"


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
        state, waiting = predicate_row(predicate, found)
        if state.startswith("met"):
            met += 1
        lines.append(f"| {predicate.number} | {predicate.name} | {state} | {waiting} |")
    lines.append("")
    lines.append(
        f"**{met} of {len(ALPHA)} predicates met.** A predicate is met when every story declaring it "
        f"is `done`. **A story declares its predicate itself**, in its own `{PREDICATE_FIELD}:` "
        "frontmatter field, so there is no list of predicate stories kept here to fall behind the "
        "board — which is what happened, and is `X-42`."
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
        f"{total} RFCs tracked. `implemented` means what "
        f"[`docs/rfc/README.md`](rfc/README.md) says it means — *{status_definition('implemented')}* — "
        f"and `rfc-report.py` is what enforces that. **`X-38` did not change the status words. It "
        f"changed the basis of the last column**: every layer now has a shipped application under it, "
        f"in place of the caveat this table carried for `core`, `services`, `transport` and `wire`, "
        f"which said no caller had been found. `{SURFACE_CHECKER}` fails the gate when a crate claims "
        f"supported surface no application reaches. The two layers that also say *path check* carry "
        f"`rfc-report.py`'s per-row check on top; the others are entered per crate, so a single row of "
        f"them is not individually attested. The column is about reachability, not about which crate "
        f"holds the code: what a row must cite is `docs/rfc/README.md`'s business, and `X-43` is where "
        f"the one row citing no code is being weighed."
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
        lines.append(
            "Both are read from committed history **union the working tree**, and that union is what "
            "makes today's row hold still: `git commit` moves a fact from the second source to the "
            "first without changing the count, so the commit that files or closes a story can carry a "
            "report of itself. It could not before `X-39`, when the day rows came from history alone "
            "and the count the report needed was created by the commit containing the report — which "
            "made the gate's `maturity` step red in most commits and never for a defect. The row is "
            "not tolerated or marked provisional, because either of those leaves it unchecked today "
            "and strictly checked tomorrow; the crossover date is the number to watch, so it is the "
            "source that changed."
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
        "- **A predicate's stories are whichever stories declare it, so what this cannot see is a "
        f"story that declares nothing.** Every predicate above is read from the `{PREDICATE_FIELD}:` "
        "field of the stories themselves; a story naming a predicate that does not exist fails the "
        "gate rather than being dropped, and a computed predicate no story declares reads "
        "**unknown** rather than met. What no script can decide is which predicate a story *should* "
        "have named — so a filer who leaves the field empty is the one remaining way a predicate "
        "reads met while a defect against it is open. That is narrower than what it replaced: the "
        "association used to live in `scripts/maturity.py`, where three defects filed against "
        "predicate 3 in one session went unrecorded because the filer had no reason to open the file "
        "(`X-42`)."
    )
    lines.append(
        "- **An absence of stories is not an absence of defects.** Predicate 4 in particular reports "
        "the absence of open stories describing a known-wrong path, which is not the same as there "
        "being none. `S-27` — a `sips:` URI dialled in cleartext — was found on the day it was filed, "
        "not by this report."
    )
    lines.append(
        "- **Today's row is a working answer, not yet a historical one.** It counts uncommitted story "
        "files as filed and uncommitted `status: done` lines as closed, which is what lets the commit "
        "that moves the table contain the table (`X-39`). A story here means what the board means by "
        "one — a file carrying frontmatter with an `id` — so a scratch note left in the directory is "
        "not a story filed today. So a dirty tree reports a day git history does not show yet: the "
        "next commit makes it true, and `--check` calls it drift if that commit does not carry the "
        "regenerated report. Every earlier day is history alone and cannot move."
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
