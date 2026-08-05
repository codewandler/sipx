#!/usr/bin/env python3
"""Generate alpha integrity and beta-announcement readiness from checked sources.

Someone asked how far sipx is from v1, and the honest first answer was that the question had no
denominator: the roadmap ran M0-M12 with a deferral list and never named 1.0, and the only `v1` in
the tree was `sipx.app.v1`, a protocol version. So predicates were written down first
(`docs/roadmap.md`), and this generates the distance to them.

**Why generated and not written.** This project has paid twice for a hand-maintained list drifting
from what it described: the gate's command list (`X-22`, which hid a red `msrv` job for five days)
and the pool-key prose (`X-24`, wrong through two changes to the type). The roadmap's own Status
block said "941 tests pass" through four releases that took the real number past 1300. A maturity
number is the *most* tempting thing to hand-maintain and the least useful when stale, because the
only decision it feeds is whether a release could responsibly be announced.

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
import hashlib
import json
import os
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
REACHABILITY_CHECKED = {"media", "security", "services"}

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

#: The corresponding story-owned association for hypothetical public-prerelease publicity. It is a
#: separate field because beta readiness and stable-API readiness answer different questions: a
#: story may block public adoption without describing a correctness defect in an alpha predicate.
ANNOUNCEMENT_FIELD = "announcement"

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
        f"it uses; `{SURFACE_CHECKER}` holds production `Supported` claims against that "
        "application's real dependency closure. A published test product may prove only its own "
        "crate through a manifest-declared, independently compiled example target; that class never "
        "widens the production closure. The gate is red when either proof disagrees. The three "
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

#: The all-or-nothing hypothetical announcement threshold for `1.0.0-beta.5`. No feature or RFC
#: percentage appears here: a truthful smaller surface is ready and an overstated larger one is not.
#: Predicate 1 is derived from every alpha predicate and therefore deliberately has no declaring
#: story of its own.
BETA = (
    Predicate(1, "Every alpha integrity predicate still holds", "derived"),
    Predicate(2, "Hostile-input, entropy and SRTCP replay invariants are executable", "computed"),
    Predicate(3, "Browser-audio negotiation is complete and fail-closed", "computed"),
    Predicate(4, "One nominated component carries every browser-media protocol safely", "computed"),
    Predicate(5, "An independent browser endpoint carries Opus in both roles", "computed"),
    Predicate(6, "Exact registry, CLI, Pages and GitHub release evidence agrees", "computed"),
)


#: Files under `docs/stories` that are not stories however they are shaped. `_TEMPLATE.md` carries a
#: frontmatter `id:` of its own, so the frontmatter test below does *not* subsume this list — both
#: halves are load-bearing, which is why they live together in one function.
NOT_STORIES = {"README.md", "_TEMPLATE.md"}

# Historical content is immutable. The report tests render repeatedly in one process, so retain the
# answer instead of spawning one `git show` per story for every assertion.
COMMITTED_STORY_CACHE = {}


def story_fields_text(name, text):
    """One named file's story frontmatter, or `None` when it is not a board story.

    **The single definition of "is a story"**, because there were briefly two. `discovery_rate` used
    to decide it from the file name alone, which made a scratch `notes.md` in `docs/stories` count as
    a story filed today — a red gate for no defect, this story's own failure mode reached from a new
    direction. A name is not enough and neither is frontmatter alone: the board's template has an
    `id:` too. A story is a file this list does not name, carrying a frontmatter block with an `id`.
    """
    if name in NOT_STORIES:
        return None
    match = re.match(r"---\n(.*?)\n---", text, re.S)
    if not match:
        return None
    fields = {}
    for line in match.group(1).splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()
    return fields if "id" in fields else None


def story_fields(path):
    """One worktree file's story frontmatter, for callers explicitly checking that file."""
    return story_fields_text(path.name, path.read_text(encoding="utf-8"))


def stories():
    """Every story in the snapshot selected by `staged_story_changes`, by id.

    With any staged story change the index is the concrete selective-commit snapshot. With none, the
    complete worktree preserves the ordinary edit/generate/stage-all workflow. The mode boundary is a
    story change in the index, never whether the generated report happens to have been staged.
    """
    found = {}
    mode = story_snapshot_mode()
    if mode == "ordinary":
        sources = [
            (path.name, path.read_text(encoding="utf-8"))
            for path in sorted(STORIES.glob("*.md"))
        ]
    else:
        names = git_lines(["ls-files", "--cached", "--", "docs/stories"])
        if names is None:
            raise SystemExit("maturity: cannot read the staged story snapshot")
        sources = []
        for name in sorted(names):
            path = pathlib.PurePosixPath(name.strip())
            if path.parent != pathlib.PurePosixPath("docs/stories") or path.suffix != ".md":
                continue
            text = git_text(["show", f":{path.as_posix()}"])
            if text is None:
                raise SystemExit(f"maturity: cannot read staged story {path}")
            sources.append((path.name, text))
    for name, text in sources:
        fields = story_fields_text(name, text)
        if fields is not None:
            found[fields["id"]] = fields
    return found


def registry():
    return tomllib.loads(REGISTRY.read_text())["rfc"]


def git_text(args):
    """A git invocation's stdout, or `None` when git cannot answer.

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
    return done.stdout


def git_lines(args):
    """A git invocation's output lines, or `None` when git cannot answer."""
    output = git_text(args)
    return None if output is None else output.splitlines()


def staged_story_changes():
    """Story paths changed between `HEAD` and the index, or `None` without git.

    An empty list selects ordinary all-changes mode. Any path selects the index, including a deletion
    or a staged non-story file, because it is the presence of a selective snapshot that matters.
    """
    return git_lines(["diff", "--cached", "HEAD", "--name-only", "--", "docs/stories"])


def report_is_staged():
    """Whether the generated report differs between `HEAD` and the index."""
    paths = git_lines(["diff", "--cached", "HEAD", "--name-only", "--", "docs/maturity.md"])
    return None if paths is None else bool(paths)


def worktree_story_changes():
    """Board stories changed outside `HEAD`, using the same content rule as `stories`."""
    tracked = git_lines(["diff", "HEAD", "--name-only", "--", "docs/stories"])
    untracked = git_lines(["ls-files", "--others", "--exclude-standard", "--", "docs/stories"])
    if tracked is None or untracked is None:
        return None
    changed = []
    for name in [*tracked, *untracked]:
        path = pathlib.PurePosixPath(name.strip())
        if path.suffix != ".md" or path.name in NOT_STORIES:
            continue
        worktree_path = ROOT / path
        worktree_fields = None
        if worktree_path.is_file():
            try:
                worktree_fields = story_fields(worktree_path)
            except (OSError, UnicodeDecodeError):
                pass
        head_text = git_text(["show", f"HEAD:{path.as_posix()}"])
        head_fields = story_fields_text(path.name, head_text) if head_text is not None else None
        if worktree_fields is not None or head_fields is not None:
            changed.append(path.as_posix())
    return changed


def story_snapshot_mode():
    """Choose ordinary worktree or selective index mode, rejecting an ambiguous report-only stage."""
    staged = staged_story_changes()
    if staged is None:
        return "ordinary"
    if staged:
        return "selective"
    report_staged = report_is_staged()
    changed = worktree_story_changes()
    if report_staged and changed:
        raise SystemExit(
            "maturity: docs/maturity.md report is staged while story changes are not; stage the "
            "selected story changes too, or unstage the report and use the ordinary all-changes mode"
        )
    return "ordinary"


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


def staged_story_facts():
    """Filed and closed counts in the index and not yet in `HEAD`.

    **Why a pre-commit snapshot is a source at all** (`X-39`). `Filed` and `Closed` come from git
    history, so the count the report must contain for the current day is created *by the commit that
    contains the report*: regenerate then commit and the report is one short, commit then regenerate
    and the report is uncommitted. No ordering satisfies it, and the gate's `maturity` step was
    therefore red in every commit that filed or closed a story — most commits — and never for a
    defect. It was regenerated twice on 2026-07-30 with nothing wrong either time.

    **The fix is the third of the three options `X-39` lists**: the day rows come from a source that
    does not move under the commit that writes them. History *union* the staged snapshot is that
    source — `git commit` relocates a fact from the second half to the first without changing the
    total. The index matters: the worktree may contain valid stories deliberately excluded from a
    selective commit, and counting them predicts a tree that will never exist in history.

    Stage story changes before generating the report, then stage the report. Unstaged and untracked
    files contribute nothing, and a staged file is read from the index even if its worktree copy has
    since changed.

    `None` when git cannot answer, matching `discovery_rate`: an unavailable rate is reported as
    unavailable and never as zero.
    """
    added = git_lines(
        ["diff", "--cached", "HEAD", "--diff-filter=A", "--name-only", "--", "docs/stories"]
    )
    changed = git_lines(["diff", "--cached", "HEAD", "--unified=0", "--", "docs/stories"])
    if added is None or changed is None:
        return None

    def is_story(line):
        """A newly staged file that the board will read as a story."""
        path = pathlib.PurePosixPath(line.strip())
        if path.suffix != ".md":
            return False
        text = git_text(["show", f":{path.as_posix()}"])
        return text is not None and story_fields_text(path.name, text) is not None

    filed = [f"filed:{line.strip()}" for line in added if is_story(line)]

    # This diff includes the full body of newly staged files, exactly as history will after commit.
    closed = closing_fact_ids(
        changed,
        lambda path: is_story(path),
    )
    return filed, closed


def worktree_story_facts():
    """Filed and closed facts for ordinary all-changes mode.

    This is used only when the index has no story change, so the worktree is the only proposed story
    snapshot. Valid untracked stories participate because a later `git add -A` will include them.
    """
    added = git_lines(["diff", "HEAD", "--diff-filter=A", "--name-only", "--", "docs/stories"])
    untracked = git_lines(["ls-files", "--others", "--exclude-standard", "--", "docs/stories"])
    changed = git_lines(["diff", "HEAD", "--unified=0", "--", "docs/stories"])
    if added is None or untracked is None or changed is None:
        return None

    def is_story(line):
        path = ROOT / line.strip()
        if path.suffix != ".md" or not path.is_file():
            return False
        try:
            return story_fields(path) is not None
        except (OSError, UnicodeDecodeError):
            return False

    untracked_stories = [line.strip() for line in untracked if is_story(line)]
    filed_names = [line.strip() for line in added if is_story(line)] + untracked_stories
    filed = [f"filed:{name}" for name in filed_names]
    closed = closing_fact_ids(changed, lambda path: is_story(path))
    for name in untracked_stories:
        lines = (ROOT / name).read_text(encoding="utf-8").splitlines()
        closed.extend(
            f"closed:{name}" for line in lines if closes_a_story(line)
        )
    return filed, closed


def pending_story_facts():
    """Facts in the deterministic ordinary or selective pre-commit snapshot."""
    mode = story_snapshot_mode()
    return staged_story_facts() if mode == "selective" else worktree_story_facts()


def closing_fact_ids(lines, is_story):
    """Closing fact identities from a zero-context patch and its resulting-file story rule."""
    path = None
    found = []
    for line in lines:
        if line.startswith("+++ b/"):
            path = line[len("+++ b/") :].strip()
        elif line.startswith("+++ /dev/null"):
            path = None
        elif path and line.startswith("+") and closes_a_story(line[1:]) and is_story(path):
            found.append(f"closed:{path}")
    return found


EVENT_DAYS_PREFIX = "<!-- maturity-event-days: "


def parse_event_days(text, source):
    """Validate and parse one generated event-date journal."""
    if text is None:
        return None
    for line in text.splitlines():
        if not line.startswith(EVENT_DAYS_PREFIX) or not line.endswith(" -->"):
            continue
        try:
            data = json.loads(line[len(EVENT_DAYS_PREFIX) : -len(" -->")])
        except json.JSONDecodeError as error:
            raise SystemExit(f"maturity: invalid event-date journal in {source}: {error}")
        if type(data) is not dict or set(data) != {"basis", "filed", "closed"}:
            raise SystemExit(
                f"maturity: invalid event-date journal in {source}: expected only basis, filed and "
                "closed"
            )
        basis = data["basis"]
        if type(basis) is not str or not re.fullmatch(r"sha256:[0-9a-f]{64}", basis):
            raise SystemExit(
                f"maturity: invalid event-date journal in {source}: basis must be a sha256 digest"
            )
        counters = []
        for kind in ("filed", "closed"):
            values = data[kind]
            if type(values) is not dict:
                raise SystemExit(
                    f"maturity: invalid event-date journal in {source}: {kind} must be an object"
                )
            counter = collections.Counter()
            for day, count in values.items():
                try:
                    parsed_day = datetime.date.fromisoformat(day)
                except (TypeError, ValueError):
                    raise SystemExit(
                        f"maturity: invalid event-date journal in {source}: {day!r} is not an "
                        "ISO YYYY-MM-DD date"
                    ) from None
                if parsed_day.isoformat() != day:
                    raise SystemExit(
                        f"maturity: invalid event-date journal in {source}: {day!r} is not an "
                        "ISO YYYY-MM-DD date"
                    )
                if type(count) is not int or count <= 0:
                    raise SystemExit(
                        f"maturity: invalid event-date journal in {source}: {kind}[{day!r}] must "
                        "be a positive integer"
                    )
                counter[day] = count
            counters.append(counter)
        return (*counters, basis)
    return None


def snapshot_event_days():
    """The journal in the index, falling back to `HEAD` before the report is first staged.

    Reading the index is what preserves a first-observed date when a generated report is staged and
    checked again after midnight. In an ordinary unstaged generation the index still contains
    `HEAD`'s report, which is the correct base journal.
    """
    text = git_text(["show", ":docs/maturity.md"])
    if text is None:
        text = git_text(["show", "HEAD:docs/maturity.md"])
    return parse_event_days(text, "the staged docs/maturity.md")


def event_days_basis(filed, closed, filed_facts, closed_facts):
    """Bind attributed dates to the multiset of semantic facts in the selected snapshot."""
    payload = {
        "closed": dict(sorted((day, count) for day, count in closed.items() if count)),
        "closed_facts": sorted(closed_facts),
        "filed": dict(sorted((day, count) for day, count in filed.items() if count)),
        "filed_facts": sorted(filed_facts),
    }
    encoded = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def event_days_line(filed, closed, basis):
    """The deterministic generated representation of the event-date journal."""
    data = {
        "basis": basis,
        "closed": dict(sorted((day, count) for day, count in closed.items() if count)),
        "filed": dict(sorted((day, count) for day, count in filed.items() if count)),
    }
    return EVENT_DAYS_PREFIX + json.dumps(data, separators=(",", ":"), sort_keys=True) + " -->"


def committed_story(commit, name, cache):
    """Whether one path is a story in one committed snapshot."""
    key = (commit, name)
    if key not in cache:
        path = pathlib.PurePosixPath(name)
        text = git_text(["show", f"{commit}:{path.as_posix()}"])
        cache[key] = text is not None and story_fields_text(path.name, text) is not None
    return cache[key]


def shallow_history():
    """Whether git's history is truncated, so filing days cannot be read out of it.

    A shallow checkout is not the absent-git case `git_text` handles: git answers, and answers
    wrongly. The grafted commit has no parent, so every story file present in it reads as *added*
    there — the filed count becomes the number of story files that exist rather than the number of
    filings that happened, and every one of them is dated to the checkout's single commit.

    Both numbers agreed for as long as every story ever filed still existed under its original path,
    and CI checked out at the default depth 1. The first renumber — `eee4394`, `P-6` refiled as
    `P-7`, two filings and one surviving file — made them differ by one, and the mismatch surfaced as
    a diagnostic accusing the journal of recording facts the snapshot did not have (`X-49`). The
    journal was right; the history it was compared against was one commit deep.
    """
    answer = git_text(["rev-parse", "--is-shallow-repository"])
    return answer is not None and answer.strip() == "true"


#: How the history walk reads a merge commit, and the whole of `X-55`'s fix.
#:
#: **The defect.** `git log -p` and `git log --diff-filter=A --name-only` both emit *nothing* for a
#: merge commit unless asked, so a fact whose only appearance is a merge commit is invisible. `M-34`'s
#: `status: done` landed in the merge that resolved its branch — "Merge impl/M-34, and close it" — so
#: the closing was in no non-merge diff, the journal came out one ahead of the snapshot, and recovering
#: took hand-editing the generated report.
#:
#: **Why counting merge diffs, rather than detecting and refusing.** `X-55` offered both and asked for
#: one. Refusing is cheaper, but this repository's history *already* contains two such closings
#: (`M-34` and `S-26`), and history is immutable — a detector would make the gate permanently red for
#: a defect no one can fix, which is predicate 3 read backwards. Counting them is also simply the
#: right answer, and it fixes an over-count in the same stroke; see below.
#:
#: **Why the walk is limited to first parents and not merely `--diff-merges=first-parent`.** `X-55`'s
#: Notes suggested `--diff-merges=first-parent` might reproduce the committed rows exactly, making
#: adoption nearly free. It does not: it takes this repository from 144 closed facts to 180 and from
#: 182 filed to 224, because `git log` walks every parent by default, so a fact on a branch is counted
#: once on the branch commit and again in the merge that brought it in. Limiting the revision walk to
#: the mainline is what makes the count right — **a story fact is an event on the first-parent
#: history**, counted exactly once wherever it was written, whether that was an ordinary commit or the
#: merge itself.
#:
#: **What that changed, and why it is a repair rather than a re-attribution.** Against the real
#: history it moves three numbers, and all three were wrong before: `M-34`'s closing and `S-26`'s
#: closing were missing, and `S-26` was counted as filed *twice* — `f67ffad` filed it on `main` while
#: `0236340` independently created the same file on a branch cut from an earlier commit, and the
#: all-parents walk counted both. Closed goes 144 -> 146, filed 182 -> 181.
#:
#: The attribution date of a fact written on a branch becomes the day it reached the mainline rather
#: than the day it was authored. That is the honest reading once a fact is defined as a mainline event,
#: and it is the same day in every case in this history.
MAINLINE_WALK = ("--first-parent", "--diff-merges=first-parent")


def history_story_fact_days():
    """Committed `(day, identity)` facts, newest first, using the board's content rule.

    Read along the first-parent history, so a fact written inside a merge commit is counted and a fact
    merged in from a branch is counted once. See `MAINLINE_WALK`.
    """
    if shallow_history():
        raise SystemExit(
            "maturity: story filing days are read from history, and this is a shallow checkout. "
            "Fetch the full history (`git fetch --unshallow`, or `fetch-depth: 0` on "
            "actions/checkout) and run this again."
        )
    cache = COMMITTED_STORY_CACHE
    filed = []
    lines = git_lines(
        [
            "log",
            "--date=short",
            "--format=C %H %ad",
            "--diff-filter=A",
            "--name-only",
            *MAINLINE_WALK,
            "--",
            "docs/stories",
        ]
    )
    if lines is None:
        return None
    commit = None
    day = None
    for line in lines:
        if line.startswith("C "):
            _, commit, day = line.split(maxsplit=2)
        elif line.strip().endswith(".md") and day and commit:
            name = line.strip()
            if committed_story(commit, name, cache):
                filed.append((day, f"filed:{name}"))

    closed = []
    lines = git_lines(
        [
            "log",
            "--date=short",
            "--format=C %H %ad",
            "-p",
            "--unified=0",
            *MAINLINE_WALK,
            "--",
            "docs/stories",
        ]
    )
    if lines is None:
        return None
    commit = None
    day = None
    path = None
    for line in lines:
        if line.startswith("C "):
            _, commit, day = line.split(maxsplit=2)
            path = None
        elif line.startswith("+++ b/"):
            path = line[len("+++ b/") :].strip()
        elif line.startswith("+++ /dev/null"):
            path = None
        elif (
            path
            and line.startswith("+")
            and closes_a_story(line[1:])
            and day
            and commit
            and committed_story(commit, path, cache)
        ):
            closed.append((day, f"closed:{path}"))
    return filed, closed


def event_day():
    """The day assigned to newly observed facts.

    `SOURCE_DATE_EPOCH` makes the boundary reproducible in tests and reproducible-build environments;
    otherwise the local calendar day is the user-facing meaning of "today" in the report.
    """
    source_epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if source_epoch is None:
        return datetime.date.today().isoformat()
    try:
        instant = datetime.datetime.fromtimestamp(int(source_epoch), tz=datetime.timezone.utc)
    except (OverflowError, ValueError):
        raise SystemExit("maturity: SOURCE_DATE_EPOCH must be an integer Unix timestamp") from None
    return instant.date().isoformat()


#: The repair for a journal that no regeneration can satisfy, named in the diagnostics that need it.
#:
#: `X-55`'s last Acceptance item. The recorded journal is a floor, so when it records facts the
#: snapshot does not have, the generator refuses instead of overwriting — which is right, and used to
#: leave no way forward but deleting the generated `maturity-event-days` line out of `docs/maturity.md`
#: by hand, staging it and regenerating. That is a reverse-engineered repair, and a hand-edited count
#: then fails the basis hash, so the only safe hand edit was deleting the whole line — which nothing
#: said. This flag is that operation, spelled out.
RESEED_FLAG = "--reseed-journal"

#: What to do about a journal the sources cannot justify. Both diagnostics end with this, because both
#: are unrecoverable by plain regeneration and neither said so.
RESEED_ADVICE = (
    f"The committed journal is read as a floor, so regenerating cannot repair this. Rebuild the date "
    f"attribution from committed history with `./scripts/maturity.py {RESEED_FLAG}`, then stage "
    f"docs/maturity.md. Do that only when the journal is what is wrong: it discards first-observed "
    f"dates and re-derives them, so a fact's day becomes its commit's day."
)


def reconcile_event_days(kind, recorded, history, pending, today):
    """Reconcile one journal counter with committed and pending fact totals."""
    recorded_total = sum(recorded.values())
    committed_total = len(history)
    maximum = committed_total + pending
    if recorded_total > maximum:
        raise SystemExit(
            f"maturity: event-date journal records {recorded_total} {kind} facts, but the snapshot "
            f"has {committed_total} committed + {pending} pending. {RESEED_ADVICE}"
        )

    if recorded_total <= committed_total:
        for day in history[: committed_total - recorded_total]:
            recorded[day] += 1
        recorded_pending = 0
    else:
        recorded_pending = recorded_total - committed_total
    for _ in range(pending - recorded_pending):
        recorded[today] += 1
    return recorded


def discovery_rate(reseed=False):
    """Stories filed and closed per recorded day, from history and the selected snapshot.

    The least obvious output here and the most useful. Burn-down is not a maturity signal while
    discovery outpaces closure: a shrinking board means the authors have stopped being surprised,
    and a growing one means the opposite however much gets done. The date the crossover becomes
    *durable* is the real marker.

    Filed is a story file being added. Closed is a `status: done` line appearing — so a story that
    is reopened and closed again counts twice, which is the honest reading of "closed on that day".

    The generated report carries the date journal that makes the attribution stable. Existing
    history was seeded from author dates; a pending fact gets the day the report first observes it.
    That recorded day survives a later commit across midnight and an amend retaining an old author
    date. See `pending_story_facts`, and `X-39` for the gate step this repaired.

    `reseed` discards the recorded journal and re-derives every date from the snapshot, which is the
    documented repair for a journal no regeneration can satisfy (`RESEED_FLAG`, and `X-55`).
    """
    history = history_story_fact_days()
    if history is None:
        return None
    history_filed_facts, history_closed_facts = history
    history_filed = [day for day, _ in history_filed_facts]
    history_closed = [day for day, _ in history_closed_facts]
    pending = pending_story_facts()
    if pending is None:
        return None
    pending_filed_facts, pending_closed_facts = pending
    pending_filed = len(pending_filed_facts)
    pending_closed = len(pending_closed_facts)
    journal = None if reseed else snapshot_event_days()
    if journal is None:
        filed = collections.Counter(history_filed)
        closed = collections.Counter(history_closed)
        recorded_basis = None
    else:
        filed, closed, recorded_basis = journal
    journal_complete = (
        sum(filed.values()) == len(history_filed) + pending_filed
        and sum(closed.values()) == len(history_closed) + pending_closed
    )
    all_filed_facts = [identity for _, identity in history_filed_facts] + pending_filed_facts
    all_closed_facts = [identity for _, identity in history_closed_facts] + pending_closed_facts
    if journal_complete and recorded_basis is not None:
        expected_basis = event_days_basis(filed, closed, all_filed_facts, all_closed_facts)
        if recorded_basis != expected_basis:
            raise SystemExit(
                "maturity: event-date journal basis does not match the facts and attributed dates. "
                + RESEED_ADVICE
            )
    today = event_day()
    filed = reconcile_event_days("filed", filed, history_filed, pending_filed, today)
    closed = reconcile_event_days("closed", closed, history_closed, pending_closed, today)
    basis = event_days_basis(filed, closed, all_filed_facts, all_closed_facts)
    return filed, closed, basis


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


def story_declarations(story_id, fields, field):
    """The numbered predicates one story declares through one frontmatter field.

    Accepts `3` and `[3, 7]`, because a defect can falsify two predicates and forcing a filer to pick
    one would leave the other reading `met` — the failure this field exists to remove. An empty field
    is a story that declares nothing, which is most of them, and a trailing `#` comment is ignored.

    A field that is not a list of numbers exits with a message rather than raising: a malformed
    frontmatter line is a thing to fix, not a traceback to read.
    """
    raw = fields.get(field, "").partition("#")[0].strip()
    if not raw:
        return ()
    numbers = []
    for token in raw.strip("[]").split(","):
        token = token.strip()
        if not token:
            continue
        if not token.isdigit():
            raise SystemExit(
                f"maturity: {story_id} has `{field}: {raw}`, and `{token}` is not a predicate "
                f"number; write `{field}: 3` or `{field}: [3, 5]`"
            )
        numbers.append(int(token))
    return tuple(numbers)


def story_predicates(story_id, fields):
    """The alpha predicates one story declares, retained as the public helper for tests."""
    return story_declarations(story_id, fields, PREDICATE_FIELD)


def predicate_stories(found, predicates=ALPHA, field=PREDICATE_FIELD, gate="alpha"):
    """Every story declaring each predicate, by predicate number.

    **The single source of the association.** Nothing else in this file may hold a list of which
    stories bear on which predicate; if it did, the two would drift and the drift is `X-42`.
    """
    known = {predicate.number for predicate in predicates}
    declared = collections.defaultdict(list)
    for story_id in sorted(found, key=story_key):
        for number in story_declarations(story_id, found[story_id], field):
            if number not in known:
                raise SystemExit(
                    f"maturity: {story_id} declares `{field}: {number}`, and there is no "
                    f"{gate} predicate {number} — `docs/roadmap.md` has "
                    f"{', '.join(str(item) for item in sorted(known))}"
                )
            declared[number].append(story_id)
    return declared


def predicate_state(
    predicate,
    found,
    predicates=ALPHA,
    field=PREDICATE_FIELD,
    gate="alpha",
):
    """Which of a predicate's stories are still open, and every story that declares it.

    Both halves are returned because the report needs to tell two different things apart: a predicate
    whose stories are all closed, and a predicate no story claims at all. Calling the second one met
    would mean deleting the last story that named a predicate looked like finishing it.
    """
    declared = predicate_stories(found, predicates, field, gate).get(predicate.number, [])
    open_stories = [
        story_id for story_id in declared if found[story_id].get("status", "ready") in OPEN
    ]
    return open_stories, declared


def predicate_row(
    predicate,
    found,
    predicates=ALPHA,
    field=PREDICATE_FIELD,
    gate="alpha",
):
    """The `State` and `Waiting on` cells for one predicate."""
    open_stories, declared = predicate_state(predicate, found, predicates, field, gate)
    if predicate.kind == "computed" and not declared:
        # Not met: unrecorded. A computed predicate is computed over stories, and there are none to
        # compute over — so the honest report is that nobody has said what would close it.
        return "**unknown**", f"no story declares `{field}: {predicate.number}`"
    if open_stories:
        return "open", ", ".join(f"`{story_id}`" for story_id in open_stories)
    return ("met" if predicate.kind == "computed" else "met (attested)"), "—"


def announcement_predicate_row(predicate, found, alpha=ALPHA):
    """One beta-announcement row, including integrity derived from the complete alpha gate."""
    if predicate.kind != "derived":
        return predicate_row(
            predicate,
            found,
            predicates=BETA,
            field=ANNOUNCEMENT_FIELD,
            gate="beta-announcement",
        )
    waiting = []
    for alpha_predicate in alpha:
        state, _ = predicate_row(alpha_predicate, found, predicates=alpha)
        if not state.startswith("met"):
            waiting.append(f"alpha predicate {alpha_predicate.number} ({state.strip('*')})")
    if waiting:
        return "open", ", ".join(waiting)
    return "met", "—"


def render(reseed=False):
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

    # ---- hypothetical prerelease publicity, deliberately separate from the stable v1 gate
    beta_met = 0
    lines.append("## Hypothetical announcement readiness for `1.0.0-beta.5`")
    lines.append("")
    lines.append("| # | Predicate | State | Waiting on |")
    lines.append("|---|---|---|---|")
    for predicate in BETA:
        state, waiting = announcement_predicate_row(predicate, found)
        if state.startswith("met"):
            beta_met += 1
        lines.append(f"| {predicate.number} | {predicate.name} | {state} | {waiting} |")
    lines.append("")
    lines.append(
        f"**{beta_met} of {len(BETA)} predicates met. All {len(BETA)} are required; this is not a weighted "
        "score.** Integrity is derived from the alpha table above. Every other association lives in "
        f"the blocking story's own `{ANNOUNCEMENT_FIELD}:` frontmatter, so the report has no second "
        "list to drift. RFC coverage is intentionally absent from this gate: a smaller truthful "
        "surface is announceable and an overstated larger one is not. This informational threshold "
        "does not authorize publicity."
    )
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
        f"supported surface no declared reachability class proves. The layers that also say *path "
        f"check* carry "
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
    rate = discovery_rate(reseed=reseed)
    if rate is None:
        lines.append(
            "Unavailable: this is read from git history, and git could not be asked. Not reported as "
            "zero, because zero filed and zero closed would read as a converged project."
        )
    else:
        filed, closed, basis = rate
        days = sorted(set(filed) | set(closed))[-10:]
        lines.append(event_days_line(filed, closed, basis))
        lines.append("")
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
            "Both are read from committed history **union a deterministic pre-commit snapshot**. With "
            "no staged story change that snapshot is the complete worktree, preserving the ordinary "
            "edit → generate → stage-all workflow. Any staged story change selects the index, so a "
            "selective commit excludes unstaged and untracked stories. The generated comment above is "
            "the event-date journal; its basis binds the dates to the filed and closed story paths, so "
            "unchanged totals cannot conceal rewritten attribution. Existing facts were seeded from "
            "commit author dates; a new fact gets the day the report first observes it. Carrying that "
            "journal in the staged report "
            "keeps the row fixed across midnight and an amend with a retained author date. A committed "
            "fact absent from the journal is still computed from history, so forgetting regeneration "
            "remains strict drift."
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
        f"story that declares nothing.** Alpha associations are read from `{PREDICATE_FIELD}:` and "
        f"beta associations from `{ANNOUNCEMENT_FIELD}:` on the stories themselves; a story naming "
        "a predicate that does not exist fails the "
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
        "- **The newest row can describe a pre-commit snapshot rather than `HEAD`.** Pending filed and "
        "closed facts are assigned the day this report first observes them, and their event-date "
        "journal is carried inside the generated region. This is deliberately not called the next "
        "commit's author date: Git has no such date before that commit exists. With a selective story "
        "snapshot, unstaged and untracked stories are excluded, so a dirty worktree can describe a "
        "different future board without changing the report of the commit currently staged (`X-39`)."
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
    parser.add_argument(
        RESEED_FLAG,
        action="store_true",
        help=(
            "discard the recorded event-date journal and re-derive every date from committed history "
            "and the pre-commit snapshot; the documented repair when the journal records facts the "
            "snapshot does not have, which no regeneration can fix"
        ),
    )
    args = parser.parse_args()

    if args.reseed_journal and args.check:
        # `--check` must never be the thing that decides the journal is wrong: it is the reader that
        # reports drift, and a reseeding check would report green while rewriting the attribution it
        # was asked to verify.
        print(
            f"maturity: {RESEED_FLAG} rewrites the journal and --check verifies it; run them "
            "separately",
            file=sys.stderr,
        )
        return 1

    generated = render(reseed=args.reseed_journal)
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
        print(
            f"maturity: {len(ALPHA)} alpha predicates and {len(BETA)} beta-announcement "
            "predicates, report current"
        )
        return 0

    write(generated)
    if args.reseed_journal:
        print(
            f"maturity: rebuilt the event-date journal in {REPORT.relative_to(ROOT)} from committed "
            "history; stage it"
        )
    else:
        print(f"maturity: wrote {REPORT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
