#!/usr/bin/env python3
"""Report stories whose Acceptance is satisfied and whose `status` was never moved.

`A-16` is why this exists. Its 834-line spec landed on 2026-08-05 in `3686d03`, which ticked seven of
its eight Acceptance rows and never ran `/track:done`, so the frontmatter still read `backlog`. Three
days later the wave-selection pass read that status, promoted the story to `ready` and dispatched it
to an implementor, who refused rather than write a second spec over a contract six downstream stories
already cite by section and vector ID. The cost of missing this is not the implementor's afternoon.
It is a second implementation landing on top of a live contract.

Frontmatter is the source of truth and the board is a view of it, which is exactly why nothing in the
repository reconciles a story's *status* with its own *Acceptance*. The gate checks that the board
agrees with the frontmatter; it cannot check that the frontmatter agrees with itself.

# The rule, and why each half of it is there

A story is reported when **all three** hold:

1. its `status` is `backlog` or `ready`;
2. at least one Acceptance row is ticked and **at most one** is outstanding;
3. that state is what the *committed* board says, not what somebody's working tree says.

**Clause 1 is what keeps this quiet during ordinary work.** Implementors tick rows as they go, so a
story being implemented has ticks in it, and a check that reported those would fire every day on
every story under work — which is the check people learn to scroll past, and that is worse than no
check because it also occupies the slot a working one would have had. The lifecycle already has a
word for *somebody is ticking these rows right now*: `in-progress`. `blocked` is excluded for the
same reason from the other direction — a story parked on a dependency is parked part-way, and
part-way is when it carries partial ticks. What is left, `backlog` and `ready`, is precisely the set
a selection pass promotes from, which is the harm being guarded against.

**Clause 2 is what keeps it quiet through deliberate partial delivery**, and it is also why the
threshold is not *every row*. Both halves are calibrated against this board's own history rather than
chosen:

- `A-16` was 7 of 8, and the eighth row was "`./scripts/gate.py` is green" — a row an implementor
  cannot honestly tick before the wave gate has run. A rule demanding a complete Acceptance would
  have been silent about the one story it was written for.
- `X-29` was landed in halves on purpose (`daf4fde`, *"Land X-29's verified half, keep the story
  open"*) and sat at 3 of 6 with `status: ready` for two commits. Two or more outstanding rows is
  real work left, and a story with real work left is legitimately open however many rows are ticked.

A row that is neither `[x]` nor `[ ]` counts as outstanding. This board uses `[~]` for a row that was
recast rather than satisfied, and reading that as satisfied would report stories whose own author
wrote down that the row was not.

**Clause 3 is the other half of the benign case.** What makes the defect a defect is that the work
reached the branch a selection pass reads from and the status did not follow. An implementation in
flight has its ticks in a working tree, or on an `impl/` branch nobody has integrated — neither is in
the committed board of the branch selection runs on, so this is silent inside an implementor's
worktree, which is the one place a noisy check would do the most damage.

# Measured against the whole recorded board

Swept over every committed state of every story on `main`'s first-parent history — 926 states across
355 story files — this rule reports **four states across three stories**, and all four are the same
defect:

    A-16  2026-08-05  3686d03  backlog  7 of 8   sat for three days, then was dispatched again
    A-16  2026-08-08  5376828  ready    7 of 8   the selection pass that promoted it
    X-12  2026-07-28  14bf31e  ready    4 of 4   closed by the next commit
    X-29  2026-07-29  f56cec2  ready    5 of 6   closed by the next commit

There are no other hits. `X-29`'s deliberate partial landing two commits earlier, at 3 of 6, is not
among them, and neither is any of the 290 stories that were closed normally.

# Why it reports rather than fails

Because two of those three were closed by the very next commit. A gate step would have been red on
`f56cec2` — the commit that *landed X-29's completed work* — for a state that was fixed minutes
later, which is predicate 3 read backwards: a red gate must mean a defect somebody has to act on.
`--strict` exists for a caller that has decided otherwise, and nothing in this repository passes it.

# Where it runs

Not only in the gate, because the gate is not where selection happens. The pre-commit hook runs it
with `--staged` whenever a commit touches `docs/stories/`, which is the earliest moment the defect
exists — `A-16` would have been named on 2026-08-05, three days before the wave read it. Run it
directly before choosing work; that is what `docs/stories` selection should be preceded by.

The limit worth stating: `git merge` does not run the pre-commit hook, so a defect created by an
integration merge alone is first reported by the next story-touching commit or by a direct run.
"""

import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
STORIES = "docs/stories"

#: Files under `docs/stories` that are not stories however they are shaped. `_TEMPLATE.md` carries a
#: frontmatter `id:` of its own, so the frontmatter rule below does not subsume this list. Kept in
#: step with `NOT_STORIES` in `maturity.py`, which is the other reader of this directory.
NOT_STORIES = {"README.md", "_TEMPLATE.md"}

#: The statuses a selection pass promotes from, and therefore the only ones where an unmoved status
#: can cause the harm. `in-progress` and `blocked` are deliberately absent — see the module docstring.
SELECTABLE = ("backlog", "ready")

#: How many Acceptance rows may still be outstanding before a story stops looking finished. One,
#: because `A-16`'s outstanding row was the gate row every story carries and two outstanding rows is
#: real work left. Calibrated against this board's history, not chosen.
MAX_OUTSTANDING = 1

#: The exit code for a run that could not read a committed board. Distinct from a finding, and
#: distinct from success: a check that read nothing must not be mistaken for a check that found
#: nothing. Matches the gate's own use of `2` for an incomplete run.
INCOMPLETE = 2

FRONTMATTER = re.compile(r"---\n(.*?)\n---", re.S)
ACCEPTANCE = re.compile(r"^##[ \t]+Acceptance[ \t]*$(.*?)(?=^##[ \t]|\Z)", re.S | re.M)
ROW = re.compile(r"^-[ \t]+\[(.)\]", re.M)


class Finding:
    """One story that reads as available work to a selection pass and is not."""

    def __init__(self, story_id, path, status, ticked, outstanding):
        self.story_id = story_id
        self.path = path
        self.status = status
        self.ticked = ticked
        self.outstanding = outstanding

    @property
    def total(self):
        return self.ticked + self.outstanding


def git(*args):
    """A git invocation's stdout, or `None` when git cannot answer.

    Absent git, or a repository with no commit in it, is not a clean board — it is no board. The
    caller reports that rather than printing nothing, because printing nothing reads as green.
    """
    try:
        done = subprocess.run(
            ["git", *args], cwd=ROOT, capture_output=True, text=True, check=False
        )
    except OSError:
        return None
    return None if done.returncode != 0 else done.stdout


def story_fields(name, text):
    """One named file's story frontmatter, or `None` when it is not a board story.

    A name is not enough and neither is frontmatter alone: the board's template has an `id:` too. A
    story is a file `NOT_STORIES` does not name, carrying a frontmatter block with an `id`. This is
    the same rule `maturity.py` applies, deliberately, so the two readers of this directory cannot
    disagree about what a story is.
    """
    if name in NOT_STORIES:
        return None
    match = FRONTMATTER.match(text)
    if not match:
        return None
    fields = {}
    for line in match.group(1).splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()
    return fields if "id" in fields else None


def acceptance_counts(text):
    """Ticked and outstanding Acceptance rows in one story's text.

    Rows are read only from the `## Acceptance` section, and only at column zero, which is where
    every row in this board sits. A `###` subheading inside the section does not end it; the next
    `##` heading does — including `## Acceptance note on the predicate`, which one story carries
    below its rows and which is prose rather than more rows.

    Outstanding is *every row that is not ticked*, so `[ ]` and this board's `[~]` both count.
    """
    section = ACCEPTANCE.search(text)
    if section is None:
        return 0, 0
    marks = ROW.findall(section.group(1))
    ticked = sum(1 for mark in marks if mark in "xX")
    return ticked, len(marks) - ticked


def looks_finished(fields, ticked, outstanding):
    """Whether one story's committed state is the shape this check reports. See the docstring."""
    return (
        fields.get("status") in SELECTABLE
        and ticked > 0
        and outstanding <= MAX_OUTSTANDING
    )


def committed_board(staged):
    """Every story in the committed board, as `(path, text)`, or `None` when there is none.

    `staged` reads the index — the snapshot the commit being written will make true, which is what
    the pre-commit hook needs, since at that moment the ticks are staged and not yet committed.
    Otherwise `HEAD` is read: the board as the branch actually stands, which is what a selection pass
    sees and what a working tree's in-flight edits must not be able to move.

    Blobs are fetched in one `cat-file --batch` rather than a `git show` per story. At 355 stories the
    difference is a second and a half on every story-touching commit, and a hook that costs that is a
    hook somebody turns off.
    """
    if staged:
        listing = git("ls-files", "--stage", "-z", "--", STORIES)
        # `<mode> <sha> <stage>\t<path>`
        parse = lambda entry: (entry.split("\t", 1)[1], entry.split()[1])
    else:
        listing = git("ls-tree", "-r", "-z", "HEAD", "--", STORIES)
        # `<mode> <type> <sha>\t<path>`
        parse = lambda entry: (entry.split("\t", 1)[1], entry.split()[2])
    if listing is None:
        return None

    wanted = {}
    for entry in listing.split("\0"):
        if not entry.strip():
            continue
        path, blob = parse(entry)
        if path.endswith(".md"):
            wanted[blob] = path
    if not wanted:
        return None

    try:
        done = subprocess.run(
            ["git", "cat-file", "--batch"],
            cwd=ROOT,
            input="\n".join(wanted).encode(),
            capture_output=True,
            check=False,
        )
    except OSError:
        return None
    if done.returncode != 0:
        return None

    board = []
    data = done.stdout
    offset = 0
    while offset < len(data):
        end = data.index(b"\n", offset)
        blob, _, size = data[offset:end].decode().partition(" ")
        offset = end + 1
        length = int(size.split()[-1])
        text = data[offset : offset + length].decode("utf-8", errors="replace")
        offset += length + 1
        board.append((wanted[blob], text))
    return board


def findings(staged=False):
    """Every story in the committed board that looks finished, or `None` when there is no board."""
    board = committed_board(staged)
    if board is None:
        return None
    found = []
    for path, text in sorted(board):
        fields = story_fields(path.rsplit("/", 1)[-1], text)
        if fields is None:
            continue
        ticked, outstanding = acceptance_counts(text)
        if looks_finished(fields, ticked, outstanding):
            found.append(
                Finding(fields["id"], path, fields["status"], ticked, outstanding)
            )
    return found


def story_key(finding):
    """Board order: by prefix, then numerically, so `X-19` does not print before `X-9`."""
    prefix, _, number = finding.story_id.partition("-")
    return (prefix, int(number) if number.isdigit() else 0, finding.story_id)


def report(found, staged):
    """Print the finding list, or say plainly that there is nothing to say."""
    snapshot = "the staged board" if staged else "the committed board"
    if not found:
        print(f"story closure: no story in {snapshot} is implemented and still open")
        return
    subject = "story" if len(found) == 1 else "stories"
    verb = "reads as available work and is not" if len(found) == 1 else (
        "read as available work and are not"
    )
    print(f"story closure: {len(found)} {subject} in {snapshot} {verb}")
    print()
    for finding in sorted(found, key=story_key):
        print(f"  {finding.story_id}  status: {finding.status}")
        print(
            f"    {finding.ticked} of {finding.total} Acceptance rows ticked, "
            f"{finding.outstanding} outstanding"
        )
        print(f"    {finding.path}")
    print()
    print(
        "A selection pass reads `status:` and will dispatch these again. Close each with "
        "`/track:done <ID>`,\nor untick the rows that are not in fact satisfied. This is a report: "
        "it does not fail the build."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--staged",
        action="store_true",
        help=(
            "read the index rather than HEAD — the board the commit being written will create, "
            "which is what the pre-commit hook reports on"
        ),
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help=(
            "exit non-zero when there is anything to report; deliberately not the default, and "
            "nothing in this repository passes it (see the module docstring)"
        ),
    )
    args = parser.parse_args()

    found = findings(staged=args.staged)
    if found is None:
        print(
            "story closure: there is no committed board to read, so nothing is claimed about it",
            file=sys.stderr,
        )
        return INCOMPLETE

    report(found, args.staged)
    return 1 if found and args.strict else 0


if __name__ == "__main__":
    sys.exit(main())
