#!/usr/bin/env python3
"""Report a `cfg`-gated item whose gate outlives every caller's — dead code on a platform the
local gate cannot build.

# The defect this exists for

`crates/sipx-cli/tests/support/strict_json.rs`'s `versioned_bytes` was gated
`#[cfg(feature = "device-audio")]`. Its only caller is gated
`#[cfg(all(feature = "device-audio", target_os = "linux"))]`. Every configuration where the feature
is on and the platform is not Linux therefore compiles the helper with nothing left to call it, and
`-D warnings` turns that dead code into an error. Both `device audio compiles` jobs — `macos-15` and
`windows-2025` — were red across every push for a day, while `./scripts/gate.py` was green and
`scripts/check-features.sh` was green including its `sipx-cli device-audio` row. Neither was wrong:
on a Linux host the caller *does* compile and the helper *is* used, so there was nothing local to
see. The failure reads as a platform problem and is a `cfg` that disagrees with its caller.

The generalisation is the thing worth guarding, not the instance:

    an item whose `cfg` is satisfied in some configuration where no use of it is,
    is dead code in that configuration.

That is a property of the *text* — two predicates and the set of lines that name the item. It needs
no cross compiler, no platform SDK and no second host, which is why it can be checked here even
though the configuration it is about cannot be built here.

`X-125` also measured what a cross compiler can do on a Linux gate host, and the answer is half:
`cargo check --target x86_64-pc-windows-gnu` builds the whole workspace and *does* catch this
instance, so it is a gate step now. `x86_64-apple-darwin` and `aarch64-apple-darwin` do not build —
`ring`'s build script needs a C compiler targeting Darwin — so the `macos-15` half of
`device-portable` is still only reachable from here, by reading. See `gate.py`'s `NOT_RUN_LOCALLY`
entry for the measurement.

# What it looks for

For every item carrying a `#[cfg(...)]`, the checker computes two sets of **required atoms** — the
conditions that must hold for a line to be compiled at all:

* the item's own, from its `cfg` plus every `cfg` enclosing it (an enclosing `mod`, an enclosing
  block, and the `cfg` on the `mod x;` declaration that pulled its file in);
* one per use site, the same way.

`all(a, b)` requires both. `any(p, q)` requires only what both branches require, so
`any(all(unix, x), all(windows, x))` requires `x` and nothing else. `not(a)` requires `not(a)`, an
atom in its own right. `target_os = "linux"` is taken to imply `unix` and `target_family = "unix"`,
and `target_os = "windows"` to imply `windows` — otherwise a correctly narrower item would be
reported for spelling its platform a different way from its callers.

An item is reported when some atom is required by **every** use of it and by the item itself is not.
That atom names a configuration in which the item compiles and no caller does.

Two directions of error, and they are not symmetric. A use this reader fails to see makes the
intersection *wider* and a false report *more* likely, so it is deliberately generous about what
counts as a use: any mention of the identifier outside the item's own body, including inside a `use`
declaration, a macro argument or a string literal. A name shared with something unrelated therefore
silences the check for that name rather than inventing a finding — the direction that ends in a
defect nobody sees rather than in a sentence nobody can act on.

# Scope — what it reads, and what it therefore cannot see

**Every `*.rs` file under `crates/`.** `src/`, `tests/`, `benches/` and `examples/` alike: the
instance was in a test-support module, but nothing about the shape is particular to tests, and a
guard over `crates/sipx-cli/tests/` would be a rule fitted to the one sample it was written from.

**An item and its uses must share a compilation unit**, and the units are `crates/<crate>/src`,
`crates/<crate>/tests`, `crates/<crate>/benches` and `crates/<crate>/examples`. That is as far as a
non-exported item can be seen, so a use outside it cannot be keeping the item alive. `tests/` is
taken whole rather than per test binary, which over-counts uses when two binaries share a `support/`
tree — again the direction that under-reports.

Five things it cannot see. Each is a false *negative*; none of them can manufacture a finding.

**An item that is `pub` in a library's `src/`.** It is reachable from outside the crate, so it is
never dead code and there is nothing to say about it. Under `tests/`, `benches/` and `examples/` a
`pub` item is examined like any other, because those roots export to nobody.

**An item with no use at all in its unit.** That is plain dead code, `-D warnings` already refuses
it on this host, and reporting it here would duplicate clippy while adding a second thing to keep
honest. What is caught is the item that *is* used — on Linux.

**A use that names the item indirectly**: reached through a re-export under a different name, built
by a macro from fragments, or resolved through a trait object. There is no name to match, so the
item reads as unused and is skipped by the rule above.

**Whether a configuration is reachable at all.** `all(feature = "a", feature = "b")` where the
manifest makes `b` imply `a` is not really two configurations, and this does not read manifests.
`scripts/check-features.sh` is where feature *combinations* are held to building.

**Anything that is not a `cfg`.** A lint whose outcome depends on the toolchain — `X-125`'s sibling
defect, three `#[expect(...)]` in `crates/sipx-call/tests/cancel.rs` left unfulfilled under coverage
instrumentation — has no predicate to compare and is invisible here.

Usage:
    ./scripts/check-cfg-callers.py --check            # this repository
    ./scripts/check-cfg-callers.py --check --root DIR # a tree assembled by a test
"""

import argparse
import pathlib
import re
import sys
from dataclasses import dataclass, field

#: What the check says it covers, printed on every run — green included. A guard whose scope is
#: known only to its author gets trusted past it, and this repository has shipped that failure
#: three times.
SCOPE = (
    "every *.rs under crates/ (src, tests, benches, examples); an item and its uses must share "
    "one of crates/<crate>/{src,tests,benches,examples}"
)

#: The blind spots, in the same breath as the result rather than in a file the reader has to go and
#: find. All are false negatives — see the module docstring for why none can invent a finding.
BLIND = (
    "not covered: a `pub` item in a library src/, an item with no use at all (clippy's dead_code "
    "already refuses that here), a use reached only through a rename, a macro or a trait object, "
    "and whether a configuration is reachable at all (scripts/check-features.sh)"
)

#: Below these the reader has stopped understanding the workspace rather than found a small one.
#: A checker that silently reads nothing reports nothing, forever — which is indistinguishable from
#: a clean tree, and is the failure this repository has shipped three times.
_PLAUSIBLE_FILES = 20
_PLAUSIBLE_GATES = 40
_PLAUSIBLE_ITEMS = 5

_CFG_ATTRIBUTE = re.compile(r"#\s*!?\s*\[\s*cfg\s*\(")
_ATTRIBUTE = re.compile(r"^\s*#\s*!?\s*\[")
_TOKEN = re.compile(r'\s*(?:(?P<op>[(),=])|(?P<text>"[^"]*")|(?P<word>[A-Za-z_][A-Za-z0-9_]*))')
_RAW_STRING = re.compile(r'b?r(#*)"')
_CHAR = re.compile(r"'(?:\\.|[^\\'])'")
_WORD = re.compile(r"\b\w+\b")

#: An item declaration, in the spellings this workspace writes. `const` and `unsafe` appear both as
#: modifiers and as the keyword itself, which the optional groups resolve by backtracking:
#: `const fn parse` is a `fn`, `const LIMIT` is a `const`.
_ITEM = re.compile(
    r"^\s*(?P<vis>pub\b(?:\s*\([^)]*\))?\s+)?"
    r"(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]+\"\s+)?"
    r"(?:(?P<kind>fn|struct|enum|union|trait|type|mod|const|static)\s+(?P<name>\w+)"
    r"|macro_rules\s*!\s*(?P<macro_name>\w+))"
)

#: The head of something that owns a block or is a whole declaration, rather than being one element
#: of a comma-separated list. `extent_of` needs the distinction; its docstring has the defect that
#: made it necessary.
_BLOCKISH = re.compile(
    r"^(?:pub\b(?:\s*\([^)]*\))?\s+)?"
    r"(?:default\s+|const\s+|async\s+|unsafe\s+|extern\s+(?:\"[^\"]*\"\s+)?)*"
    r"(?:fn|struct|enum|union|trait|impl|type|mod|use|static|const|macro_rules|macro)\b"
)

#: Operating systems whose `target_os` implies `unix`. Not exhaustive over rustc's list, and does
#: not need to be: an omission makes a narrower item look broader, which ends in a report somebody
#: reads rather than a silence nobody does.
_UNIX_LIKE = {
    "linux",
    "macos",
    "ios",
    "tvos",
    "watchos",
    "visionos",
    "android",
    "freebsd",
    "netbsd",
    "openbsd",
    "dragonfly",
    "solaris",
    "illumos",
    "haiku",
    "redox",
    "fuchsia",
    "hurd",
    "aix",
    "nto",
    "emscripten",
}

#: Files that root a module tree in the directory they sit in, rather than in a subdirectory named
#: after themselves. Every other file `foo.rs` puts its `mod bar;` at `foo/bar.rs`.
_TREE_ROOTS = {"lib.rs", "main.rs", "mod.rs"}


# --------------------------------------------------------------------------------------------
# Reading Rust as text
# --------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Source:
    """One file in the three views this reader needs.

    Separating them is not tidiness. `crates/sipx-cli/tests/cli.rs` builds an ALSA configuration
    with `format!`, and that string contains `}}` — so a brace counter reading the raw text closes
    the enclosing test three hundred lines early, every `cfg` around it stops applying, and the
    caller of `versioned_bytes` reads as requiring nothing at all. The instance this whole checker
    was written for was invisible to it until the literals were taken out of the structure.
    """

    #: As written. Reported line numbers index this.
    lines: tuple[str, ...]
    #: Comments and the *contents* of string and character literals blanked to spaces, so braces,
    #: semicolons and commas inside them cannot move a block boundary. Column-aligned with `lines`.
    structural: tuple[str, ...]
    #: Comments blanked, literals kept. What a name is searched in — a mention inside a string
    #: counts as a use, deliberately, because an unseen use is the error direction that reports.
    text: tuple[str, ...]


def read_source(text: str) -> Source:
    """Split one file into its three views in a single pass over the characters.

    A line-at-a-time regex cannot do this: a Rust string literal crosses lines, both by `\\` at the
    end of a line and by being raw, and a block comment crosses lines too. The state carried
    between lines is exactly what a line-local reader gets wrong.
    """
    lines = text.splitlines()
    structural: list[str] = []
    kept: list[str] = []
    mode = "code"
    hashes = 0
    for line in lines:
        blank: list[str] = []
        shown: list[str] = []
        at = 0
        width = len(line)
        while at < width:
            character = line[at]
            if mode == "code":
                if character == "/" and line.startswith("//", at):
                    blank.append(" " * (width - at))
                    shown.append(" " * (width - at))
                    break
                if character == "/" and line.startswith("/*", at):
                    mode, at = "block", at + 2
                    blank.append("  ")
                    shown.append("  ")
                    continue
                raw = _RAW_STRING.match(line, at) if character in "rb" else None
                if raw is not None:
                    hashes, mode = len(raw.group(1)), "raw"
                    blank.append(" " * (raw.end() - at))
                    shown.append(line[at : raw.end()])
                    at = raw.end()
                    continue
                if character == '"':
                    mode, at = "string", at + 1
                    blank.append(" ")
                    shown.append('"')
                    continue
                if character == "'":
                    # A char literal, or a lifetime. `'a` is code; `'a'` is not.
                    literal = _CHAR.match(line, at)
                    if literal is not None:
                        blank.append(" " * (literal.end() - at))
                        shown.append(literal.group(0))
                        at = literal.end()
                        continue
                blank.append(character)
                shown.append(character)
                at += 1
            elif mode == "block":
                if character == "*" and line.startswith("*/", at):
                    mode, at = "code", at + 2
                    blank.append("  ")
                    shown.append("  ")
                    continue
                blank.append(" ")
                shown.append(" ")
                at += 1
            elif mode == "string":
                if character == "\\" and at + 1 < width:
                    blank.append("  ")
                    shown.append(line[at : at + 2])
                    at += 2
                    continue
                if character == '"':
                    mode, at = "code", at + 1
                    blank.append(" ")
                    shown.append('"')
                    continue
                blank.append(" ")
                shown.append(character)
                at += 1
            else:
                closing = '"' + "#" * hashes
                if line.startswith(closing, at):
                    mode, at = "code", at + len(closing)
                    blank.append(" " * len(closing))
                    shown.append(closing)
                    continue
                blank.append(" ")
                shown.append(character)
                at += 1
        structural.append("".join(blank))
        kept.append("".join(shown))
    return Source(tuple(lines), tuple(structural), tuple(kept))


# --------------------------------------------------------------------------------------------
# cfg predicates
# --------------------------------------------------------------------------------------------


def _lex(text: str) -> list[str] | None:
    """`text` as tokens, or `None` if something in it is not a `cfg` predicate.

    `None` rather than a partial parse: a predicate this cannot read must widen nothing and narrow
    nothing, and a half-read `all(...)` would do both.
    """
    tokens: list[str] = []
    position = 0
    text = text.strip()
    while position < len(text):
        found = _TOKEN.match(text, position)
        if found is None:
            return None
        position = found.end()
        tokens.append(found.group("op") or found.group("text") or found.group("word"))
    return tokens


def _parse(tokens: list[str], at: int) -> tuple[tuple, int]:
    """One predicate from `tokens`, and where it ended."""
    if at >= len(tokens):
        raise ValueError("a predicate ended early")
    head = tokens[at]
    at += 1
    if head in ("all", "any", "not") and at < len(tokens) and tokens[at] == "(":
        at += 1
        children = []
        while at < len(tokens) and tokens[at] != ")":
            child, at = _parse(tokens, at)
            children.append(child)
            if at < len(tokens) and tokens[at] == ",":
                at += 1
        if at >= len(tokens):
            raise ValueError("an unclosed predicate")
        at += 1
        if head == "not":
            if len(children) != 1:
                raise ValueError("`not` takes one predicate")
            return ("not", children[0]), at
        return (head, children), at
    if at + 1 < len(tokens) and tokens[at] == "=":
        return ("atom", f"{head} = {tokens[at + 1]}"), at + 2
    return ("atom", head), at


def required(node: tuple) -> set[str]:
    """The atoms that must hold in every configuration satisfying this predicate.

    `any(...)` contributes only what all of its branches agree on, which is what makes
    `any(all(unix, x), all(windows, x))` require `x` and not `unix`.
    """
    kind = node[0]
    if kind == "atom":
        return {node[1]}
    if kind == "all":
        found: set[str] = set()
        for child in node[1]:
            found |= required(child)
        return found
    if kind == "any":
        sets = [required(child) for child in node[1]]
        return set.intersection(*sets) if sets else set()
    inner = node[1]
    return {f"not({inner[1]})"} if inner[0] == "atom" else set()


def atoms_of(predicate: str) -> tuple[set[str], bool]:
    """The required atoms of one `cfg(...)` body, and whether the body could be read at all.

    The second half matters because an empty atom set has two very different causes.
    `any(target_os = "linux", target_os = "macos")` requires *nothing* — no single atom holds in
    every configuration satisfying it — and that item is still dead on Windows, so it has to be
    examined. A predicate this reader cannot parse also yields nothing, and examining that one
    would compare its callers against an empty set and report whatever they happen to share. So an
    unreadable predicate is skipped and an empty-but-read one is not.
    """
    tokens = _lex(predicate)
    if tokens is None:
        return set(), False
    try:
        node, end = _parse(tokens, 0)
    except (ValueError, IndexError):
        return set(), False
    return (required(node), True) if end == len(tokens) else (set(), False)


def implied(atoms: set[str]) -> set[str]:
    """`atoms` plus what they entail, so a narrower item is not reported for spelling it otherwise.

    `#[cfg(target_os = "linux")]` on the item and `#[cfg(unix)]` on every caller is an item that is
    *narrower* than its callers, which is not this rule's defect and must not read as one.
    """
    found = set(atoms)
    for atom in atoms:
        named = re.fullmatch(r'target_os = "(\w+)"', atom)
        if named is not None and named.group(1) in _UNIX_LIKE:
            found |= {"unix", 'target_family = "unix"'}
        elif named is not None and named.group(1) == "windows":
            found |= {"windows", 'target_family = "windows"'}
        if atom in ("unix", 'target_family = "unix"'):
            found |= {"unix", 'target_family = "unix"'}
        if atom in ("windows", 'target_family = "windows"'):
            found |= {"windows", 'target_family = "windows"'}
        if atom == 'target_arch = "wasm32"':
            found.add('target_family = "wasm"')
    return found


# --------------------------------------------------------------------------------------------
# Reading one file
# --------------------------------------------------------------------------------------------


@dataclass(frozen=True)
class Scope:
    """The lines one `cfg` attribute governs, and what it requires."""

    #: The attribute's own line, zero-based; the report points here.
    at: int
    #: First and last line the attribute governs, inclusive and zero-based.
    start: int
    end: int
    atoms: frozenset[str]
    #: The predicate as written, for the report — a reader needs the spelling, not the atom set.
    written: str
    #: Line and column the attribute applies to, or `None` for a file-wide `#![cfg(...)]`. This is
    #: where a declaration is looked for, so a gate on a struct field cannot be read as a gate on
    #: whatever item happens to follow it.
    subject: tuple[int, int] | None = None
    #: Whether the predicate parsed. An unreadable one contributes no atoms, and an item under one
    #: is left alone rather than compared against a set that means "not understood".
    readable: bool = True


@dataclass(frozen=True)
class Item:
    """One `cfg`-gated declaration, reduced to what decides whether it can outlive its callers."""

    name: str
    kind: str
    path: pathlib.Path
    scope: Scope
    #: Its own `cfg` and every `cfg` around it. This is the set a caller is compared against; the
    #: gate inherited from a `mod x;` declaration is added later, once the module tree is known.
    atoms: frozenset[str]
    #: Whether it is `pub` with no restriction. In a library's `src/` that puts it outside this
    #: rule; under a test, bench or example root it means nothing, because those export to nobody.
    exported: bool
    #: A bare `mod x;`, whose gate has to reach the file it names.
    is_external_mod: bool
    #: Whether every `cfg` around it parsed. One that did not is left alone — see `atoms_of`.
    readable: bool


def balanced(source: Source, at: int, opened: int) -> tuple[str, int, int] | None:
    """The text between the parentheses opened at `structural[at][opened]`, and where it closes.

    Attributes wrap: rustfmt breaks a long `#[cfg(all(...))]` across lines, and a reader of one
    line would see an unbalanced fragment and have to guess.
    """
    depth = 0
    collected: list[str] = []
    for index in range(at, len(source.lines)):
        # The predicate's own string literals are wanted here — `feature = "device-audio"` is the
        # whole atom — so this reads the view that keeps them.
        body = source.text[index]
        start = opened if index == at else 0
        for column in range(start, len(body)):
            character = body[column]
            if character == "(":
                depth += 1
                if depth == 1:
                    continue
            elif character == ")":
                depth -= 1
                if depth == 0:
                    return "".join(collected), index, column
            if depth >= 1:
                collected.append(character)
        collected.append(" ")
    return None


def subject_of(source: Source, after: int, column: int) -> tuple[int, int] | None:
    """The first line and column the attribute closing at `[after][column]` applies to.

    Usually the next line; `#[cfg(unix)] use std::os::unix::fs;` puts it on the same one, and a
    stack of attributes or a doc comment puts it several below. The column matters because a
    same-line subject has the attribute's own text in front of it, and reading that text as part of
    the subject would count the predicate's own parentheses.
    """
    rest = source.structural[after][column + 1 :]
    closed = rest.find("]")
    if closed >= 0 and rest[closed + 1 :].strip():
        return after, column + 1 + closed + 1
    index = after + 1
    while index < len(source.lines):
        body = source.structural[index]
        if not body.strip():
            index += 1
            continue
        if _ATTRIBUTE.match(body):
            # An attribute in the way may itself span lines, and its continuation lines are not
            # subjects. `cli.rs` writes `#[allow(\n clippy::too_many_lines,\n reason = "…"\n)]`
            # between the `cfg` and the test it gates; a reader that stepped over only the first
            # line took `clippy::too_many_lines,` for the subject, gave the `cfg` an extent of four
            # lines, and left the test body — including the call this checker exists to see —
            # outside every gate around it.
            depth = 0
            while index < len(source.lines):
                depth += source.structural[index].count("[") - source.structural[index].count("]")
                index += 1
                if depth <= 0:
                    break
            continue
        return index, len(body) - len(body.lstrip())
    return None


def extent_of(source: Source, subject: int, column: int) -> int:
    """The last line the subject starting at `[subject][column]` occupies.

    Two rules, because a `cfg` sits on two different kinds of thing and confusing them is how this
    reader would invent findings. An **item** — `fn`, `struct`, `impl`, `use`, a `mod` — runs to its
    closing brace or its semicolon. An **element** — a function parameter, a struct field, an enum
    variant, a match arm, a statement — runs to the comma that ends it, and leaving the brackets it
    sits inside ends it too.

    Getting that wrong is not academic. `#[cfg(feature = "wss")] client: Option<…>` is a parameter
    of `sipx-transport`'s `dial_ws`; read as an item, its `cfg` covered the whole function body, so
    every call in that body looked like it required `wss` and `dial_ws` was reported as outliving a
    caller that requires exactly what it does.

    Brace counting rather than a parser, for the same reason the rest of this repository's readers
    use it: the question asked is only "which lines are inside this one". Braces in string literals
    are already gone — see `Source`.
    """
    head = source.structural[subject][column:].lstrip()
    is_item = _BLOCKISH.match(head) is not None
    braces = 0
    brackets = 0
    started = False
    for index in range(subject, len(source.lines)):
        body = source.structural[index][column if index == subject else 0 :]
        for character in body:
            if character in "([":
                brackets += 1
            elif character in ")]":
                brackets -= 1
                # The element ran out before its own comma did — a trailing parameter, or the last
                # field before a closing brace. Either way this is where it ends.
                if not is_item and not started and brackets < 0:
                    return index
            elif character == "{":
                braces += 1
                started = True
            elif character == "}":
                braces -= 1
                if started and braces <= 0:
                    return index
                if not is_item and not started and braces < 0:
                    return index
            elif character == ";" and not started and braces <= 0 and brackets <= 0:
                return index
            elif (
                character == ","
                and not is_item
                and not started
                and braces <= 0
                and brackets <= 0
            ):
                return index
        if started and braces <= 0:
            return index
    return len(source.lines) - 1


def scopes_in(source: Source) -> list[Scope]:
    """Every `cfg` attribute in one file, with the lines it governs."""
    found: list[Scope] = []
    for at, line in enumerate(source.structural):
        for attribute in _CFG_ATTRIBUTE.finditer(line):
            read = balanced(source, at, attribute.end() - 1)
            if read is None:
                continue
            predicate, closes, column = read
            # An inner attribute — `#![cfg(...)]` — governs the whole file rather than one item.
            inner = "!" in line[attribute.start() : attribute.end()]
            subject = None if inner else subject_of(source, closes, column)
            if inner:
                start, end = 0, len(source.lines) - 1
            elif subject is None:
                continue
            else:
                start, end = at, extent_of(source, subject[0], subject[1])
            atoms, readable = atoms_of(predicate)
            found.append(
                Scope(
                    at,
                    start,
                    end,
                    frozenset(atoms),
                    predicate.strip(),
                    subject,
                    readable,
                )
            )
    return found


def enclosing(scopes: list[Scope], line: int) -> set[str]:
    """Everything that must hold for `line` to be compiled, from this file alone."""
    found: set[str] = set()
    for scope in scopes:
        if scope.start <= line <= scope.end:
            found |= scope.atoms
    return found


def all_readable(scopes: list[Scope], line: int) -> bool:
    """Whether every `cfg` around `line` is one this reader understood."""
    return all(scope.readable for scope in scopes if scope.start <= line <= scope.end)


def items_in(path: pathlib.Path, source: Source, scopes: list[Scope]) -> list[Item]:
    """The `cfg`-gated declarations in one file.

    Exactly the subject of each attribute, never a line near it. A `cfg` on a struct field is not a
    `cfg` on the struct, and a reader that searched forward for the next declaration would report
    the wrong name under the wrong gate.
    """
    found: list[Item] = []
    for scope in scopes:
        if scope.subject is None:
            continue
        line, column = scope.subject
        rest = source.structural[line][column:]
        declaration = _ITEM.match(rest)
        if declaration is None:
            continue
        kind = declaration.group("kind") or "macro_rules"
        found.append(
            Item(
                name=declaration.group("name") or declaration.group("macro_name"),
                kind=kind,
                path=path,
                scope=scope,
                atoms=frozenset(enclosing(scopes, line)),
                exported=(declaration.group("vis") or "").strip() == "pub",
                is_external_mod=kind == "mod" and ";" in rest,
                readable=all_readable(scopes, line),
            )
        )
    return found


# --------------------------------------------------------------------------------------------
# The workspace
# --------------------------------------------------------------------------------------------


def unit_of(path: pathlib.Path, root: pathlib.Path) -> tuple[str, str] | None:
    """Which compilation unit a file belongs to, or `None` if it is not in one.

    As far as a non-exported item can be seen, and no further — a use outside the unit cannot be
    what keeps the item alive.
    """
    parts = path.relative_to(root).parts
    if len(parts) < 3 or parts[0] != "crates":
        return None
    return (parts[1], parts[2]) if parts[2] in ("src", "tests", "benches", "examples") else None


def module_file(declared_in: pathlib.Path, name: str) -> list[pathlib.Path]:
    """Where `mod name;` written in `declared_in` looks for its file."""
    directory = (
        declared_in.parent
        if declared_in.name in _TREE_ROOTS
        else declared_in.parent / declared_in.stem
    )
    return [directory / f"{name}.rs", directory / name / "mod.rs"]


def file_gates(per_file: dict[pathlib.Path, list[Item]]) -> dict[pathlib.Path, set[str]]:
    """What a file inherits from the `mod x;` declarations that pull it in, transitively.

    `crates/sipx-transport/src/lib.rs` gates `pub mod quic;` on the `quic` feature, so every item
    in `quic.rs` requires that feature whether or not it says so. Omitting this would make all of
    them look broader than their callers and report every one.
    """
    gates: dict[pathlib.Path, set[str]] = {path: set() for path in per_file}
    # A module tree is shallow, so a few passes reach the fixed point; the bound is there so that a
    # `mod` cycle this reader mis-resolves cannot spin.
    for _ in range(8):
        settled = True
        for path, items in per_file.items():
            for item in items:
                if not item.is_external_mod:
                    continue
                inherited = gates[path] | set(item.atoms)
                for candidate in module_file(path, item.name):
                    if candidate in gates and not inherited <= gates[candidate]:
                        gates[candidate] |= inherited
                        settled = False
        if settled:
            break
    return gates


@dataclass
class Finding:
    item: Item
    #: The atoms every use requires and the item does not — the configurations it outlives them in.
    missing: tuple[str, ...]
    uses: tuple[tuple[pathlib.Path, int, tuple[str, ...]], ...]


@dataclass
class Report:
    findings: list[Finding] = field(default_factory=list)
    files: int = 0
    #: Every `cfg` attribute read. Zero would mean the reader found no predicates at all.
    gates: int = 0
    #: `cfg`-gated items whose name is used somewhere in their unit — the population the rule is
    #: actually applied to, and the number that says whether it was applied to anything.
    examined: int = 0


def mentions_in(
    unit: list[pathlib.Path], read: dict[pathlib.Path, Source]
) -> dict[str, list[tuple[pathlib.Path, int]]]:
    """Every identifier in a unit, with the lines that write it.

    Built once per unit rather than searched once per item: the workspace has hundreds of gated
    items and a `tests/` unit is tens of thousands of lines, and a regex pass per item over all of
    them costs seconds where one tokenising pass costs milliseconds. What it indexes is the view
    that keeps string literals — see `uses_of` for why that generosity is the safe direction.
    """
    index: dict[str, list[tuple[pathlib.Path, int]]] = {}
    for path in unit:
        for at, line in enumerate(read[path].text):
            for word in set(_WORD.findall(line)):
                index.setdefault(word, []).append((path, at))
    return index


def uses_of(
    item: Item,
    mentions: dict[str, list[tuple[pathlib.Path, int]]],
    scopes: dict[pathlib.Path, list[Scope]],
    gates: dict[pathlib.Path, set[str]],
) -> list[tuple[pathlib.Path, int, set[str]]]:
    """Every line in the unit that names the item, outside the item's own declaration.

    Deliberately generous — a `use` declaration, a macro argument and a mention inside a string all
    count. A use this cannot see makes the intersection below wider and a finding false; a use it
    over-counts only makes the intersection narrower and the check quieter. Only one of those two
    errors ends in a sentence somebody has to answer.
    """
    found = []
    for path, at in mentions.get(item.name, ()):
        if path == item.path and item.scope.start <= at <= item.scope.end:
            continue
        found.append((path, at, enclosing(scopes[path], at) | gates[path]))
    return sorted(found, key=lambda use: (str(use[0]), use[1]))


def check(root: pathlib.Path) -> Report:
    """Apply the rule to every unit under `crates/`."""
    report = Report()
    read = {
        path: read_source(path.read_text(encoding="utf-8"))
        for path in sorted((root / "crates").rglob("*.rs"))
    }
    scopes = {path: scopes_in(source) for path, source in read.items()}
    per_file = {path: items_in(path, read[path], scopes[path]) for path in read}
    gates = file_gates(per_file)
    units: dict[tuple[str, str], list[pathlib.Path]] = {}
    for path in read:
        unit = unit_of(path, root)
        if unit is not None:
            units.setdefault(unit, []).append(path)

    report.files = len(read)
    report.gates = sum(len(found) for found in scopes.values())
    mentions = {unit: mentions_in(paths, read) for unit, paths in units.items()}

    for path, items in sorted(per_file.items()):
        unit = unit_of(path, root)
        if unit is None:
            continue
        for item in items:
            # A `pub` item in a library's `src/` is reachable from outside the crate, so it is
            # never dead code. Under a test, bench or example root nothing is exported and `pub`
            # says nothing — which is exactly where the instance was.
            if item.exported and unit[1] == "src":
                continue
            # A predicate this reader could not parse says nothing about which configurations the
            # item exists in, and comparing callers against "not understood" would report whatever
            # they happen to share. See `atoms_of`.
            if not item.readable:
                continue
            mine = set(item.atoms) | gates[path]
            found = uses_of(item, mentions[unit], scopes, gates)
            if not found:
                # Plain dead code. `-D warnings` refuses it on this host already, and duplicating
                # clippy would add a second thing to keep honest for no new coverage.
                continue
            report.examined += 1
            shared: set[str] = set.intersection(*[atoms for _, _, atoms in found])
            missing = sorted(shared - implied(mine))
            if missing:
                report.findings.append(
                    Finding(
                        item=item,
                        missing=tuple(missing),
                        uses=tuple(
                            (where, line, tuple(sorted(atoms))) for where, line, atoms in found
                        ),
                    )
                )
    return report


def describe(finding: Finding, root: pathlib.Path) -> str:
    """One finding, said so a reader can act on it without opening the checker."""
    item = finding.item
    where = item.path.relative_to(root)
    missing = ", ".join(f"`{atom}`" for atom in finding.missing)
    lines = [
        f"{where}:{item.scope.at + 1}: `{item.name}` is gated `cfg({item.scope.written})`, and "
        f"every one of its {len(finding.uses)} use sites also requires {missing}.",
        f"      In a configuration where this item's gate holds and {missing} does not, it is "
        f"compiled with nothing to use it — dead code, which `-D warnings` makes an error. A "
        f"Linux gate host compiles the caller, so nothing else here can see it.",
    ]
    for path, at, atoms in finding.uses[:4]:
        stated = ", ".join(atoms) if atoms else "nothing"
        lines.append(f"      used at {path.relative_to(root)}:{at + 1}, which requires {stated}")
    if len(finding.uses) > 4:
        lines.append(f"      … and {len(finding.uses) - 4} more")
    lines.append(
        f"      Give this item its callers' full gate — {missing} included — or widen a caller's."
    )
    return "\n".join(lines)


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
    counted = (
        f"{report.files} Rust files, {report.gates} cfg attributes, "
        f"{report.examined} gated items with a use"
    )
    if args.root is None and (
        report.files < _PLAUSIBLE_FILES
        or report.gates < _PLAUSIBLE_GATES
        or report.examined < _PLAUSIBLE_ITEMS
    ):
        print(
            f"read {counted} under {root / 'crates'} — too little to have applied the rule to this "
            f"workspace, so a clean result would mean the reader has drifted rather than that the "
            f"tree is clean.\n  scope: {SCOPE}",
            file=sys.stderr,
        )
        return 1
    if report.findings:
        print(
            "a cfg-gated item outlives every caller's gate, so it is dead code in a configuration "
            "this host does not build:",
            file=sys.stderr,
        )
        for finding in report.findings:
            print(f"  {describe(finding, root)}", file=sys.stderr)
        print(f"  scope: {SCOPE}\n  {BLIND}", file=sys.stderr)
        return 1
    print(f"cfg reach: {counted}, none outliving its callers\n  scope: {SCOPE}\n  {BLIND}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
