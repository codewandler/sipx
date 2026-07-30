#!/usr/bin/env python3
"""Hold the workspace to the rule `docs/designs/media.md` states and nothing enforced.

    a fixed wall-clock duration may bound a failure, or define silence.
    It may not stand in for a happens-before.

That sentence is normative, it is old, and it has been swept for twice. `X-28` cleared the media
path; `X-29` swept the rest of the workspace; `0.12.0`'s changelog says "no test in the workspace
now asserts after a fixed sleep". Nothing ran. So the sentence was true on the day it was written
and held by nobody afterwards, and two violations landed inside the very next wave — one window
serving as both a start deadline and an inter-packet gap (`X-40`), and two interval assertions that
failed 14 of 20 runs at 6x CPU oversubscription (`M-33`). Both were caught by a human reading a
diff, which is the mechanism this repository replaces with a check everywhere else it can.

# What it looks for: a shape, not a spelling

`sleep` is one way to write a wall-clock wait. `std::thread::sleep`, `tokio::time::sleep_until`,
`tokio::time::advance`, a bare `interval.tick()`, a spin on `Instant::now() < deadline` and a
two-line private helper called anything at all are others, and a guard that greps `sleep(` invites
the next author to write the identical defect one row down — the "rule fitted to the data it was
tested on" failure this project keeps warning about. So what is identified is the **shape**: a
suspension point whose *only completion condition is a clock reading a fixed duration*.

Three things follow from defining it that way.

**The path is irrelevant.** `tokio::time::sleep`, `time::sleep`, a re-export and `sleep` imported
bare are one shape, because the check reads the call's own name and not the module it came through.

**A wrapper is a rename with an extra step.** A function whose body is a bare wait, which asserts
nothing and awaits nothing else, *is* a bare wait, and calls to it are wait sites. That is the
spelling a grep can never see, and it is one hop away from the obvious one.

**A wait that is not on a clock is not a wait.** Two shapes complete on the event rather than on
the duration, and both are recognised structurally, with no comment asked of them:

    the duration bounds an awaited event — `timeout`, `timeout_at`, a `select!` arm beside
    another arm. The clock is the loser's branch, which is what a bound on failure is;

    the wait is the poll interval of a loop that re-tests a condition and leaves when it holds.
    The loop is the happens-before; the interval only decides how often it is asked.

Those two are the fixes the rule asks for, so they cost nothing. A guard that made the correct
thing more expensive than the wrong one gets switched off by whoever hits it second.

A **paused clock** is not a wall clock either. Under `#[tokio::test(start_paused = true)]` — or
after `tokio::time::pause()` — time moves only when the runtime is asked to move it, so
`advance(61s)` is deterministic and the entire family of failures this rule is about cannot occur.
Reporting those would teach implementors that the honest fix costs them a comment.

# When it asks for a reason, and what counts as one

A bare clock wait is reported when an **assertion depends on it**: an `assert!`, `assert_eq!`,
`assert_ne!` or `panic!` reachable from the wait without leaving the block it sits in. That is the
shape of the defect — stimulate, let a duration pass, claim the stimulus arrived — and it is the
one that fails under load while proving nothing about the diff under test.

The reason has to be a **classification**, not a description. "Give the far end a moment to catch
up" leaves the next reader exactly where the unannotated version did; what the rule needs to know
is *which question this duration answers*. The categories are below, each with the spellings it is
recognised by, and one of them has to appear in a comment attached to the wait — the contiguous
`//` block above it, or a trailing comment on the line — or in the doc comment of the named
constant the duration comes from. The call site, in other words.

**There is no suppression list, under any name.** No path is special, no file is blessed, and
nothing here can be silenced from a distance: `X-26`'s guard passed with a phantom claim in place
because the claim was in a fourth file nobody had listed, `X-35` was built without a list for that
reason, and this is the same decision. A wait this reports is either classifiable — in which case
the classification belongs at the site, where the next reader is — or it is the defect.

# The categories

The first three are the design document's own, and the fourth is the one `X-28` and `X-29` both
left sites for and neither named. Each is a claim a reviewer can falsify by reading the site, which
is the point of making the author write it down rather than of the words themselves.

**A bound on failure.** How long this side waits before concluding the thing is not coming.
`record_at_least`'s `within`, `collect_digits`' `within`, `DELIVERY_BOUND`, `SIGNALLING_BOUND`. Set
an order of magnitude above the honest answer rather than close to it, because it is not a window
to measure in. Load lengthens the real wait, and a bound set generously absorbs that.

**A definition of silence.** How long a hole has to be for "it has stopped" to be true.
`record_until_idle`'s `idle`, `collect_digits`' `gap`, and every window a test opens to assert that
*nothing* happened. `X-40` is the caveat rather than the exception: an idle window must not also be
a start deadline, because a single duration beaten by whichever of the two questions is slower on
the day produces an empty result and not a degraded one. The assertion under one of these is
negative, so load lengthens the window and can only make it fail — never pass wrongly.

**The clock is the measurement.** The assertion is about *which* duration elapsed, so there is
nothing to wait for and nothing to poll. `crates/sipx-cli/tests/cli.rs` separates its own 3 s
schedule from 64*T1's 32 s with a 12 s comparison: load can only push the number up, which is the
direction that fails.

**Ordering a stimulus.** The wait separates two things the test itself injects — a 180 before a
200, a header block before its body, a peer going away before the answer is written — rather than
waiting for something to arrive. A longer wait preserves that order rather than breaking it, which
is what makes it safe, and the site has to say what it looked for and why nothing observable sits
between the two. It is the weakest of the four and the one to argue with in review: the question a
reader should ask is "what would you have waited for", and where there is an answer, the answer is
the fix.

# What it cannot see, stated rather than implied

Three limits, all deliberate, because a guard whose edges are undocumented gets trusted past them.

**It reads blocks, not data flow.** The assertion has to be reachable from the wait without leaving
its block. A wait whose consequence is asserted three functions away is not reported, and there is
no dataflow analysis here that could find it. What is caught is the shape both regressions had, and
the shape almost every instance in this workspace has.

**It cannot tell whether a stated category is true.** Nothing mechanical can. What it enforces is
that the claim is *made*, at the line, in the diff — which is what turns "somebody happened to read
this carefully" into "a reviewer was handed a falsifiable sentence".

**It says nothing about a duration's value, or about one duration doing two jobs.** A five
millisecond bound on failure is correctly classified and still far too short, and `X-40`'s defect —
one window spent on both "has it started" and "has it ended" — is invisible to a reader that sees
durations rather than what each is for. That one is answered by the API instead: `record_until_idle`
and `record_at_least` are two methods, and `collect_digits` takes two durations, precisely so that
the confusion cannot be written.

# Why this and not a run under load

`M-33` measured its two interval assertions failing 14 of 20 runs at 6x CPU oversubscription, 1 of
8 at 5x and 0 of 12 at 2x, which is what makes the family worth a check at all. It is also the
argument against making the *check* a loaded run: a detector that fires probabilistically turns a
green gate into no evidence and a red one into a re-run, which is the habit `X-34` was filed
against. Manufacturing that load would need a CPU busy loop, which `AGENTS.md`'s fifth
non-negotiable forbids outright. So the gradient stays what it was — how a defect is measured once
it is suspected — and finding it is done here, deterministically, in milliseconds.

Usage:
    ./scripts/check-fixed-sleep.py --check            # this repository
    ./scripts/check-fixed-sleep.py --check --root DIR # a tree assembled by a test
"""

import argparse
import pathlib
import re
import sys
from dataclasses import dataclass, field
from typing import NamedTuple


class Category(NamedTuple):
    """One question a fixed wall-clock duration is allowed to answer."""

    #: What to call it in a report.
    name: str
    #: How a site says it. Matched case-insensitively against the comment at the call site, and
    #: deliberately narrow: this is a classification drawn from a closed set, not a description, so
    #: the spellings are the design document's own words rather than anything that sounds similar.
    written: str
    #: What claiming it means, printed when a site claims none, so the sentence a reader needs is
    #: in the failure rather than in a file they have to go and find.
    means: str


CATEGORIES = {
    "bound": Category(
        "a bound on failure",
        r"bounds?\s+on\s+failure|bounds?\s+(?:a|the|its)\s+failure|bound\s+on\s+how\s+long",
        "how long this side waits before concluding the thing is not coming — set an order of "
        "magnitude above the honest answer, never close to it",
    ),
    "silence": Category(
        "a definition of silence",
        r"definitions?\s+of\s+silence|defines?\s+silence|silence\s+is\s+defined",
        "how long a hole has to be for `it has stopped` to be true — the assertion under it is "
        "negative, so load lengthens the window and can only make it fail",
    ),
    "measurement": Category(
        "the clock is the measurement",
        r"clock\s+is\s+(?:itself\s+)?the\s+measurement"
        r"|\bis\b\W{0,3}(?:itself\s+)?the\s+measurement",
        "the assertion is about which duration elapsed, so there is nothing to wait for and load "
        "can only push the number in the direction that fails",
    ),
    "ordering": Category(
        "ordering a stimulus",
        r"order(?:s|ing)?\s+(?:a\s+stimulus|two\s+stimuli|the\s+stimuli|two\s+inputs)",
        "the wait separates two things this test injects rather than waiting for an arrival, and "
        "a longer wait preserves that order rather than breaking it — say what you looked for",
    ),
}

_CLASSIFIED = re.compile("|".join(category.written for category in CATEGORIES.values()), re.I)

#: A call that suspends on a clock, whatever it was reached through. `\b` rather than an anchored
#: path, because `tokio::time::sleep`, `time::sleep`, an imported bare `sleep` and `self.sleep` are
#: one shape and the module it arrived through is not the question.
_WAIT_CALL = re.compile(r"\b(?:sleep|sleep_until|sleep_for|park_timeout|delay_for)\s*\(")

#: The virtual-clock jump. Narrower than the calls above because `advance` is a common method name;
#: it is the `time::` that makes it the runtime's clock.
_ADVANCE = re.compile(r"\btime::advance\s*\(")

#: A bare tick. The completion condition is the interval's own clock, which is the same shape as a
#: sleep in a loop, spelled as a method.
_TICK = re.compile(r"\.tick\s*\(\s*\)")

#: A spin on the clock, written by hand. Only the shape with no body: a `while now < deadline` that
#: awaits or tests something inside it is a poll, and polls are the fix rather than the defect.
_SPIN = re.compile(r"\bwhile\b.*(?:Instant::now\s*\(\s*\)|\.elapsed\s*\(\s*\))\s*[<>]")

#: A **relative** timeout, which starts again on every iteration. The distinction from
#: `timeout_at` is the whole of `X-40`: an absolute deadline bounds the loop once, and a relative
#: one bounds each pass — so in a loop head it is the gap between arrivals *and*, on the first
#: pass, the deadline for the stream to start at all.
_RELATIVE_TIMEOUT = re.compile(r"\btimeout\s*\(")

#: Where an assertion is written, in the spellings this workspace uses.
_ASSERTION = re.compile(r"\bassert(?:_eq|_ne|_matches)?!|\bpanic!|\bdebug_assert(?:_eq|_ne)?!")

#: The clock read *inside* an assertion — the third category's own shape, and the one with no wait
#: in it at all.
_ELAPSED = re.compile(r"\.elapsed\s*\(\s*\)")

_FN = re.compile(r"\bfn\s+(?P<name>\w+)")
_LOOP_HEAD = re.compile(r"^\s*(?:\}\s*)?(?:loop\s*\{|while\b|for\b)")
_LEAVES_EARLY = re.compile(r"\b(?:break|return)\b|\?;")
_PAUSED = re.compile(r"start_paused\s*=\s*true|\btime::pause\s*\(\s*\)")
_CONSTANT = re.compile(r"\b([A-Z][A-Z0-9_]{2,})\b")

#: Below this the reader has stopped understanding the workspace rather than found a small one. A
#: checker that silently reads nothing reports nothing, forever — the failure `X-35` found in the
#: guard before it and `gate.py` guards against in its own workflow parser.
_PLAUSIBLE_FILES = 20


def code(line: str) -> str:
    """A line with its comment removed, so prose about sleeping is not sleeping."""
    return line.split("//", 1)[0]


def is_comment(line: str) -> bool:
    return line.lstrip().startswith("//")


@dataclass
class Site:
    """One place the clock is waited on, or read inside an assertion."""

    path: pathlib.Path
    #: Zero-based, so it indexes the file's lines; reported one-based.
    line: int
    #: What shape found it, named the way the report should name it.
    shape: str
    text: str
    #: Whether it suspends on the clock. The third category does not — its clock read is the
    #: assertion itself — so the structural excuses that apply to a wait do not apply to it.
    is_wait: bool = True
    #: What to do about it instead of classifying it, in this shape's own terms. A report that
    #: only offers "say which category it is" teaches the reader that the classification is the
    #: remedy, and it is the second-best one.
    remedy: str = (
        "Wait for the thing instead — a count, a channel, a condition polled in a loop — or say"
    )


@dataclass
class Report:
    problems: list[str] = field(default_factory=list)
    #: Every wall-clock wait the checker recognised, classified or not — the number that says
    #: whether it read the workspace at all.
    waits: int = 0
    #: Those carrying a stated category. Zero would mean the reasons this demands are not being
    #: written, whatever the problem count says.
    classified: int = 0
    files: int = 0


def functions(lines: list[str]) -> list[tuple[int, int]]:
    """Every function body in a file, as (first line, last line) inclusive and zero-based.

    Brace counting rather than a parser: the question asked of the answer is only "which lines
    belong to the same function as this one", and a `{` inside a string literal would have to be
    unbalanced to move it.
    """
    found = []
    index = 0
    while index < len(lines):
        if _FN.search(code(lines[index])):
            end = block_end(lines, index)
            if end is not None:
                found.append((index, end))
                index = end
        index += 1
    return found


def block_end(lines: list[str], at: int) -> int | None:
    """Where the block opened at or after `at` closes, or `None` if it never opens.

    `None` is the answer for a signature with no body — a trait method's `fn ready(&self) -> bool;`
    — and it has to be distinguishable from "closes at the end of the file", because a reader that
    treated a declaration as a function stretching to the last line would put every later function
    inside it and lose their spans.
    """
    depth = 0
    opened = False
    for end in range(at, len(lines)):
        body = code(lines[end])
        if not opened and ";" in body.split("{", 1)[0] and "{" not in body:
            return None
        depth += body.count("{") - body.count("}")
        if "{" in body:
            opened = True
        if opened and depth <= 0:
            return end
    return None


def is_causal(lines: list[str], at: int) -> bool:
    """Whether this wait's completion condition is an event rather than the clock.

    Two shapes qualify, and both are structural — nothing is written down, because writing
    something down would be a cost on the correct version. See the module docstring.
    """
    depth = 0
    for index in range(at, -1, -1):
        body = code(lines[index])
        depth += body.count("}") - body.count("{")
        if depth >= 0:
            continue
        opener = body.strip()
        depth = 0
        # A `select!` arm: the clock is one branch and an event is another, so whichever happens
        # first ends the wait. That is what a bound on failure is, written as a race.
        if "select!" in opener:
            return True
        if _LOOP_HEAD.search(opener):
            # A loop that re-tests a condition is the happens-before; the wait inside it only
            # decides how often the question is asked. `while <condition>` tests it in the head,
            # and `loop`/`for` have to leave early on something for the same to be true.
            head = opener
            if head.lstrip().startswith("while") and not re.search(
                r"Instant::now\s*\(\s*\)|\.elapsed\s*\(\s*\)", head
            ):
                return True
            body_before = "\n".join(code(line) for line in lines[index + 1 : at])
            if _LEAVES_EARLY.search(body_before):
                return True
        if _FN.search(opener):
            return False
    return False


def is_a_spin(lines: list[str], at: int) -> bool:
    """Whether a `while now < deadline` loop is a wait on the clock and nothing else.

    The same head opens both the defect and its fix. `while now < deadline {}` waits on the clock;
    `while now < deadline { match timeout_at(deadline, recv()).await { … break } }` waits on the
    event and uses the clock to give up, which is a poll with a bound on failure and is exactly
    what this rule asks people to write. What tells them apart is the body: a spin awaits nothing
    and leaves on nothing.
    """
    end = block_end(lines, at)
    if end is None:
        return True
    inside = "\n".join(code(line) for line in lines[at:end + 1])
    return ".await" not in inside and not _LEAVES_EARLY.search(inside)


def is_paused(lines: list[str], span: tuple[int, int]) -> bool:
    """Whether this function runs on a virtual clock, in which case none of this applies.

    The attribute sits above the function, so the four lines before it are read too — that is
    where `#[tokio::test(start_paused = true)]` is.
    """
    start, end = span
    window = lines[max(0, start - 4) : end + 1]
    return any(_PAUSED.search(line) for line in window)


def assertion_depends_on(lines: list[str], at: int, end: int) -> bool:
    """Whether an assertion is reachable from this line without leaving its block.

    That is the shape of the defect: stimulate, let a duration pass, claim the stimulus arrived.
    A wait with nothing asserted after it in its own block is pacing or a stimulus in its own
    right, and this rule has nothing to say about it.
    """
    depth = 0
    for index in range(at + 1, end + 1):
        body = code(lines[index])
        if _ASSERTION.search(body):
            return True
        depth += body.count("{") - body.count("}")
        if depth < 0:
            return False
    return False


def stated_reason(lines: list[str], at: int) -> str:
    """The comment attached to a site: the block above it, plus anything trailing on the line.

    Contiguous, because a comment two paragraphs up is about something else. Nothing else counts —
    a reason has to be where the next reader of this line will be.

    Returned as one line with the comment markers stripped, and in the order it was written. Both
    matter: rustfmt wraps prose at column 100, so "it *is* the measurement" routinely straddles a
    line break, and a reader that joined the lines bottom-up would see every such phrase backwards.
    """
    above = []
    index = at - 1
    while index >= 0 and is_comment(lines[index]):
        above.append(lines[index].lstrip().lstrip("/").strip())
        index -= 1
    parts = list(reversed(above))
    trailing = lines[at].split("//", 1)
    if len(trailing) == 2:
        parts.append(trailing[1].strip())
    return " ".join(parts)


def declared_constants(lines: list[str]) -> dict[str, str]:
    """Every `const NAME` in a file, with the comment block above it.

    A named constant is the other half of "at the call site": `DELIVERY_BOUND` is the reason,
    written once where the constant is declared, and repeating it at forty call sites would make
    the forty-first the one that says something else. Collected across the whole workspace, because
    `SETTLE_PAST_TIMERS` is declared in `sipx-testkit` and named in `sipx-call`'s soak — a reader
    that only looked in the file in front of it would ask that site to restate a paragraph.
    """
    found = {}
    for index, line in enumerate(lines):
        declaration = re.search(r"\bconst\s+([A-Z][A-Z0-9_]{2,})\b", code(line))
        if not declaration:
            continue
        above, doc = index - 1, []
        while above >= 0 and is_comment(lines[above]):
            doc.append(lines[above].lstrip().lstrip("/").strip())
            above -= 1
        found[declaration.group(1)] = " ".join(reversed(doc))
    return found


def constant_documentation(text: str, documented: dict[str, str]) -> str:
    """What the constants named on this line say about themselves."""
    return " ".join(documented.get(name, "") for name in _CONSTANT.findall(text))


def wrappers(lines: list[str], spans: list[tuple[int, int]]) -> set[str]:
    """Functions that are a bare wait wearing a different name.

    The one spelling a grep can never see. A function qualifies when it waits on the clock,
    asserts nothing, and awaits nothing else — so calling it is indistinguishable from writing the
    wait inline, which means it has to be treated as one.
    """
    found = set()
    for start, end in spans:
        body = [code(line) for line in lines[start : end + 1]]
        joined = "\n".join(body)
        # A helper that pauses the runtime's clock hands its callers a virtual one, so calling it
        # is not a wall-clock wait however it is spelled. `pass_the_refresh_deadline` in
        # `update.rs` is `pause(); advance(46s); resume()`, which is the honest fix rather than
        # the defect.
        if _PAUSED.search(joined):
            continue
        waiting = [
            index
            for index, line in enumerate(body)
            if _WAIT_CALL.search(line) or _ADVANCE.search(line) or _TICK.search(line)
        ]
        if not waiting or _ASSERTION.search(joined):
            continue
        awaits = [index for index, line in enumerate(body) if ".await" in line]
        if any(index not in waiting for index in awaits):
            continue
        name = _FN.search(body[0])
        if name:
            found.add(name.group("name"))
    return found


def sites(path: pathlib.Path, lines: list[str]) -> list[Site]:
    """Every place in one file where a clock decides whether an assertion holds."""
    spans = functions(lines)
    named = wrappers(lines, spans)
    called = (
        re.compile(r"\b(?:" + "|".join(re.escape(name) for name in sorted(named)) + r")\s*\(")
        if named
        else None
    )
    found: list[Site] = []
    for at, raw in enumerate(lines):
        if is_comment(raw):
            continue
        body = code(raw)
        # A declaration is not a call. `async fn sleep_until(deadline: …)` defines the shape; the
        # sites that matter are the ones that use it.
        if _FN.search(body):
            continue
        shape = ""
        if _WAIT_CALL.search(body):
            shape = "a wall-clock wait"
        elif _ADVANCE.search(body):
            shape = "a clock advance"
        elif _TICK.search(body):
            shape = "a bare interval tick"
        elif _SPIN.search(body) and is_a_spin(lines, at):
            shape = "a hand-rolled deadline spin"
        elif called is not None and called.search(body):
            shape = "a wait behind a private helper"
        if shape:
            shape += " stands between a stimulus and an assertion"
            found.append(Site(path, at, shape, body.strip()))
    found += elapsed_sites(path, lines)
    found += idle_loop_sites(path, lines)
    return found


def idle_loop_sites(path: pathlib.Path, lines: list[str]) -> list[Site]:
    """Loops whose every pass is bounded by a fresh relative timeout — `X-40`'s shape exactly.

    `while let Ok(Some(x)) = timeout(window, recv()).await` reads as "keep taking until it goes
    quiet", and it is two questions wearing one duration: on the first pass `window` is how long the
    stream has to *start*, and on every later pass it is how long a gap means it has *ended*.
    Neither is a property of the data — both are properties of how fast the machine happens to be —
    and the two are slow in different places, so whichever is tighter on the day loses. The result
    is not a short collection but an **empty** one, because the loop ends before its first
    iteration: `sipx answer` produced a valid recording of zero samples that way, and the interop
    harness reported "no audio came back" on a call that carried it.

    Two shapes are not this and are not reported. `timeout_at(deadline, …)` bounds the whole loop
    once, which is a bound on failure and the thing `record_at_least` was written to be. And a
    duration held in a variable the body **reassigns** is two durations spelled economically —
    `let mut window = 10s; while … timeout(window, …) { … window = 600ms; }` is exactly the fix
    `X-40` made, so recognising it is what keeps the guard from charging for its own remedy.
    """
    found = []
    for at, raw in enumerate(lines):
        if is_comment(raw) or not _LOOP_HEAD.search(code(raw)):
            continue
        # The head may wrap: rustfmt routinely breaks `while let Ok(Some(p)) =` from the
        # `timeout(…)` that follows it, and a reader of one line would see neither half.
        head = ""
        for index in range(at, min(at + 4, len(lines))):
            head += code(lines[index])
            if "{" in code(lines[index]):
                break
        if "timeout_at" in head or not _RELATIVE_TIMEOUT.search(head):
            continue
        bound = re.search(r"\btimeout\s*\(\s*([A-Za-z_]\w*)\s*,", head)
        end = block_end(lines, at)
        if bound is not None and end is not None:
            body = "\n".join(code(line) for line in lines[at + 1 : end + 1])
            if re.search(rf"\b{re.escape(bound.group(1))}\s*=[^=]", body):
                continue
        found.append(
            Site(
                path,
                at,
                "one duration bounds every pass of this loop, so it is both how long the stream "
                "has to start and how long a gap means it has ended",
                code(raw).strip(),
                is_wait=False,
                remedy=(
                    "Give the first pass its own deadline — a variable the body reassigns after "
                    "the first arrival, which is what `X-40` did here — or say"
                ),
            )
        )
    return found


def elapsed_sites(path: pathlib.Path, lines: list[str]) -> list[Site]:
    """Clock reads inside an assertion — the third category's shape, which waits for nothing.

    Held to the same standard as a wait, and for the same reason: `elapsed() < 12s` is either the
    measurement the test is about or a wall clock being asked to stand in for an arrival, and the
    two look identical until somebody says which.
    """
    found = []
    reported = -99
    for at, raw in enumerate(lines):
        if is_comment(raw):
            continue
        body = code(raw)
        if not _ELAPSED.search(body):
            continue
        # One assertion, one site. `assert!(started.elapsed() < D, "{:?}", started.elapsed())` reads
        # the clock twice for one claim, and reporting it twice would ask for the reason twice.
        if at - reported <= 3:
            continue
        # The assertion may open on this line or a few above it, since the macro's arguments are
        # routinely wrapped by rustfmt. The site is the line the macro opens on: that is where the
        # comment explaining it sits, and where a reader looking for the claim starts.
        opener = next(
            (
                index
                for index in range(at, max(0, at - 4) - 1, -1)
                if _ASSERTION.search(code(lines[index]))
            ),
            None,
        )
        if opener is None:
            continue
        # Inside a poll loop this is the loop's own bound on failure, checked structurally like
        # every other one: the loop is the happens-before and the clock is how it gives up.
        if is_causal(lines, at):
            continue
        reported = at
        found.append(
            Site(
                path,
                opener,
                "an assertion is made on a clock reading",
                body.strip(),
                is_wait=False,
            )
        )
    return found


def problems_in(
    path: pathlib.Path, lines: list[str], where: str, documented: dict[str, str]
) -> tuple[list[str], int, int]:
    """Every unclassified clock-decided assertion in a file, and what was counted getting there."""
    spans = functions(lines)
    problems: list[str] = []
    waits = 0
    classified = 0
    for site in sites(path, lines):
        span = next(
            ((start, end) for start, end in spans if start <= site.line <= end),
            (0, len(lines) - 1),
        )
        if is_paused(lines, span):
            continue
        if site.is_wait:
            if is_causal(lines, site.line):
                continue
            if not assertion_depends_on(lines, site.line, span[1]):
                continue
        waits += 1
        reason = (
            stated_reason(lines, site.line)
            + " "
            + constant_documentation(site.text, documented)
        )
        if _CLASSIFIED.search(reason):
            classified += 1
            continue
        problems.append(
            f"{where}:{site.line + 1}: {site.shape}, and this site does not say which question "
            f"its duration answers.\n"
            f"      {site.text}\n"
            f"      {site.remedy} at this line which of these it is:\n"
            + "\n".join(
                f"        {category.name}: {category.means}" for category in CATEGORIES.values()
            )
        )
    return problems, waits, classified


def check(root: pathlib.Path) -> Report:
    """Read every Rust file in the workspace's crates. `src/` as well as `tests/`.

    Both, deliberately. `X-40`'s instance of this defect was written in production code —
    `record_until_idle` spent one window on a start deadline and an end-of-stream gap — so a guard
    over test directories would have missed the one that shipped. And it would have missed most of
    the tests too: this workspace keeps a large share of its suite in `#[cfg(test)]` modules beside
    the code, and `crates/sipx-media/src/session.rs` alone holds more fixed-duration waits than any
    file under a `tests/` directory.
    """
    report = Report()
    read = {
        path: path.read_text(encoding="utf-8").splitlines()
        for path in sorted((root / "crates").rglob("*.rs"))
    }
    documented: dict[str, str] = {}
    for lines in read.values():
        documented.update(declared_constants(lines))
    for path, lines in read.items():
        where = path.relative_to(root)
        problems, waits, classified = problems_in(path, lines, str(where), documented)
        report.problems += problems
        report.waits += waits
        report.classified += classified
        report.files += 1
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--check", action="store_true", help="report and exit non-zero on a problem"
    )
    parser.add_argument("--root", default=None, help="tree to check (default: this repository)")
    args = parser.parse_args()
    if not args.check:
        parser.error("nothing to do but --check")

    root = pathlib.Path(args.root) if args.root else pathlib.Path(__file__).resolve().parent.parent
    report = check(root)
    if args.root is None and report.files < _PLAUSIBLE_FILES:
        print(
            f"read only {report.files} Rust files under {root / 'crates'}; the reader has drifted "
            f"from the workspace's shape and would report nothing whatever it contains",
            file=sys.stderr,
        )
        return 1
    if report.problems:
        print(
            "a fixed wall-clock duration is standing in for a happens-before "
            "(docs/designs/media.md):",
            file=sys.stderr,
        )
        for problem in report.problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(
        f"{report.files} Rust files, {report.waits} clock-decided assertions, "
        f"{report.classified} of them saying which question the duration answers"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
