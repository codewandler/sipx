#!/usr/bin/env python3
"""Hold each crate's declared stability against what the shipped application actually uses.

Alpha predicate 1 is *no claim outlives its caller*. Three stories tried to check it and each
measured the same hole. `X-30` read evidence paths and demoted three rows; `X-33` widened that to
`security` and demoted two more, and both wrote down the same limit — **a path check is satisfied by
citing a file whose relevant branch is dead**. `X-37` then declined to build the cross-crate caller
check its two predecessors had named as the successor, on the grounds that the cheap version of it
would be fitted to the three registry rows that motivated it, which is the exact failure the whole
lineage keeps warning about.

What none of them could do is say whether a capability is *worth* selecting. A check cannot: it can
only find capabilities that are mentioned. An application can, by needing them or not needing them.

So the reachable-from-a-call surface is **defined** as what the shipped application uses, and this
script reads that definition off the workspace. It is not a better grep — it takes a different input.
Its root is a program (`crates/sipx-app`, story `X-38`) that either compiles and carries a call or
does not exist, and a program has no dead branch it can cite in its own defence.

## What is compared, and where each side comes from

**The used side is the application's dependency closure**, from the Cargo manifests: start at the
application and follow the workspace's own `sipx-*` entries transitively. `[dev-dependencies]` are
deliberately excluded — a dev-dependency is what the *tests* reach, and a test is not a caller. That
is the whole reason the tests could not settle this predicate themselves.

**The declared side is what the crates say about themselves**, from the `# Stability` sections `A-8`
put in all eleven published crates and the per-module `**Experimental** (`A-8`)` markers under them.
Neither side is a list kept in this file. Adding a name here would make this the hand-maintained
register the project has already paid for twice (`X-22`'s gate list, `X-24`'s pool-key prose).

## The three things it asserts

1. **Nothing may be declared *Supported* that no path from the application reaches.** Reaching means
   both a manifest edge and a real one: some crate in the closure has to *name* the crate in code, so
   a dependency nobody imports does not launder a claim.
2. **Nothing the application reaches may be marked *Experimental*.** An experimental item exists
   because no caller has ever constrained its shape; the moment one does, the marker is false. The
   answer is to graduate it with a `CHANGELOG.md` entry — see `README.md`'s stability rule — and
   never to stop the caller.
3. **The experimental list may not be empty.** An application that needed the whole stack would be a
   claim, and by this project's standards claims get checked.

## Its limits, stated because a check that hides them is the thing it replaced

- **Assertion 1 runs at crate granularity, because that is the granularity the declarations have.**
  Outside the seven module markers, a `# Stability` section is prose about a crate. So a crate enters
  the surface whole, and a supported *module* nothing reaches is not caught. Per-module declarations
  would let this tighten; that is a successor, not a thing to fake here by parsing English.
- **A reference is attributed to the crate that contains it, not to the branch that runs it.** Inside
  the closure this inherits exactly the dead-branch weakness `X-30` recorded. What is different is
  the root: the closure starts at a program, so nothing reaches the surface without an application
  needing it, which is the half that was missing.
- **Macros and generated code hide references.** A path assembled by a macro is invisible here, and a
  crate reached only that way would read as unreached.
- **One application is one opinion.** `APPLICATIONS` is a tuple for that reason: a second
  implementation disagreeing widens the surface by joining it.
"""

import pathlib
import re
import sys
import tomllib
from typing import NamedTuple

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

#: The applications whose use defines the reachable surface.
#:
#: A tuple and not a constant, because the rule this implements says a second implementation
#: disagreeing *widens* the surface rather than being told it is wrong. Adding a root here is how that
#: happens for something inside the workspace; for something outside it, the item graduates and gets a
#: `CHANGELOG.md` entry, which is the same widening recorded where downstream readers look.
#:
#: `sipx-cli` is deliberately **not** here. It is a real application and a real caller, but it was
#: already shipped and in the tree through every release the caveat this closes stood over, so a
#: surface defined by it would have been satisfied before anything changed. `X-38` asks for an
#: application that is not the test suite and not the CLI, and this is the check that would fail if it
#: were quietly swapped back for either.
APPLICATIONS = ("sipx-app",)

#: A module marked experimental at the item, in the form `A-8` settled on. The story reference is part
#: of the pattern on purpose: the crate-root prose carries `**Experimental**` too, and matching that
#: would read every crate's own summary of itself as a per-module marker.
_MARKER = re.compile(r"^//! \*\*Experimental\*\* \(`A-8`\)", re.M)


def _glossary(word: str) -> str:
    """The shared line that *defines* one of the two words, rather than claiming it.

    Every crate carries both under its `# Stability` heading, so a line opening this way is a glossary
    entry and not a statement about the crate.

    Callers read the documentation line by line against this, and that is not a style preference. The
    first version used one multiline pattern, `^//!\\s+(?!- \\*\\*Supported\\*\\* —)`, and because `\\s`
    matches a newline it began at the bare `//!` line *above* the glossary, stepped over it, and read
    every crate's glossary as a claim — reporting `sipx-app-protocol`, which declares itself wholly
    experimental, as claiming supported surface. A checker that errs toward over-claiming is worse than
    no checker, because the thing it is here to catch is an over-claim.
    """
    return f"- **{word}** —"


def crate_path(name: str) -> str:
    """A crate's name as Rust spells it in a path: `sipx-call` is `sipx_call::`."""
    return name.replace("-", "_")


def where(path: pathlib.Path) -> str:
    """A path to name in a message: relative when it can be.

    Relative so a problem reads `crates/sipx-media/src/dtls/mod.rs` rather than an absolute path, but
    a path outside the tree must not crash the checker — the tests point `CRATES` at a fabricated
    workspace under `/tmp`, and a reader that raised there could not be tested on fixtures at all.
    """
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def published() -> list[str]:
    """Every crate in the workspace that `cargo publish` would upload.

    Read from the manifests rather than listed here, so a crate added to the workspace joins this
    check by existing instead of by somebody remembering.
    """
    found = []
    for directory in sorted(CRATES.iterdir()):
        manifest = directory / "Cargo.toml"
        if directory.is_dir() and manifest.exists():
            package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
            if package.get("publish", True) is not False:
                found.append(directory.name)
    if not found:
        raise ValueError(f"read no publishable crates under {CRATES.relative_to(ROOT)}")
    return found


class Edge(NamedTuple):
    """One workspace dependency, with what decides whether it is compiled at all."""

    name: str
    #: An optional dependency exists only if a feature turns it on.
    optional: bool
    #: Whether the edge takes the dependency's `default` feature.
    default_features: bool
    #: Features the edge asks for by name.
    features: tuple[str, ...]


def manifest(crate: str) -> dict:
    return tomllib.loads((CRATES / crate / "Cargo.toml").read_text(encoding="utf-8"))


def dependency_edges(crate: str) -> list[Edge]:
    """The workspace's own crates that `crate` depends on to build.

    `[dev-dependencies]` are excluded, and that exclusion is the point rather than an oversight: what
    the test suite reaches is what a test reaches, and this predicate is about callers. That is why
    the suite could never settle this predicate itself.
    """
    edges = []
    for name, spec in manifest(crate).get("dependencies", {}).items():
        if not name.startswith("sipx-"):
            continue
        table = spec if isinstance(spec, dict) else {}
        edges.append(
            Edge(
                name=name,
                optional=bool(table.get("optional", False)),
                default_features=bool(table.get("default-features", True)),
                features=tuple(table.get("features", ())),
            )
        )
    return edges


def feature_map(crate: str) -> dict[str, list[str]]:
    """A crate's `[features]` table."""
    return manifest(crate).get("features", {})


def close_features(crate: str, wanted: set[str]) -> set[str]:
    """A feature set closed under the crate's own feature table.

    Only activations naming a feature of *this* crate are followed. `dep:x` turns a dependency on and
    `x/y` turns a feature on in one, and neither is a feature of the crate doing the asking.
    """
    table = feature_map(crate)
    enabled = set(wanted)
    pending = list(enabled)
    while pending:
        feature = pending.pop()
        for activation in table.get(feature, ()):
            if activation.startswith("dep:") or "/" in activation:
                continue
            if activation not in enabled:
                enabled.add(activation)
                pending.append(activation)
    return enabled


def activated(crate: str, enabled: set[str], edge: Edge) -> bool:
    """Whether a dependency is compiled at all, given the features enabled on `crate`."""
    if not edge.optional:
        return True
    table = feature_map(crate)
    # An optional dependency also creates an implicit feature of its own name.
    if edge.name in enabled:
        return True
    for feature in enabled:
        for activation in table.get(feature, ()):
            if activation in (f"dep:{edge.name}", edge.name):
                return True
            if activation.startswith(f"{edge.name}/"):
                return True
    return False


def asked_of(crate: str, enabled: set[str], name: str) -> set[str]:
    """Features that `crate`'s own enabled features turn on in dependency `name` via `name/feature`."""
    table = feature_map(crate)
    return {
        activation.split("/", 1)[1]
        for feature in enabled
        for activation in table.get(feature, ())
        if activation.startswith(f"{name}/")
    }


def resolve(roots: tuple[str, ...]) -> dict[str, set[str]]:
    """Every workspace crate the applications reach, and the features enabled on each.

    **Features are part of selection, and that is the whole point of doing this properly** (`X-38`).
    An earlier version of this walked dependency names and ignored features on the grounds that the
    gate builds `--all-features`, so everything is compiled. That reasoning confused *compiled* with
    *selectable*, and it is exactly the confusion alpha predicate 1 exists to remove: Opus is
    implemented, tested and compiled by the gate, and no shipped binary can turn it on. A surface
    defined by what `--all-features` builds would have called it reachable, which is the over-claim.

    So the roots are resolved as they are *shipped* — with their default features — and a capability
    behind a feature no application enables is not on the surface. Reaching for `cargo metadata`
    would be more authoritative, but the CI job this runs in installs no Rust toolchain, and a check
    that silently degrades when its oracle is missing is worse than one that reads the manifests.
    """
    enabled: dict[str, set[str]] = {}
    for root in roots:
        enabled[root] = close_features(root, {"default"} if "default" in feature_map(root) else set())

    changed = True
    while changed:
        changed = False
        for crate in list(enabled):
            closed = close_features(crate, enabled[crate])
            if closed != enabled[crate]:
                enabled[crate] = closed
                changed = True
            for edge in dependency_edges(crate):
                if not activated(crate, enabled[crate], edge):
                    continue
                wanted = set(edge.features) | asked_of(crate, enabled[crate], edge.name)
                if edge.default_features and "default" in feature_map(edge.name):
                    wanted.add("default")
                if edge.name not in enabled:
                    enabled[edge.name] = set()
                    changed = True
                if not wanted <= enabled[edge.name]:
                    enabled[edge.name] |= wanted
                    changed = True
    return enabled


def closure(roots: tuple[str, ...]) -> set[str]:
    """Every workspace crate the applications reach, as shipped."""
    return set(resolve(roots))


def code(text: str) -> str:
    """A source file with its test module cut off.

    The same cut `check-audio-claims.py` makes, for the same reason and one more: a test naming
    `sipx_media::bridge` is a test, and reading it as a caller would let the suite promote its own
    fixtures into the supported surface.
    """
    return text.partition("#[cfg(test)]")[0]


def sources(crate: str) -> dict[pathlib.Path, str]:
    """A crate's library sources, test modules removed.

    `tests/`, `examples/` and `benches/` are outside `src/` and so are absent here by construction.
    An example is documentation and a test is a test; neither is the application.
    """
    found = {}
    for path in sorted((CRATES / crate / "src").rglob("*.rs")):
        found[path] = code(path.read_text(encoding="utf-8"))
    return found


def entry(crate: str) -> pathlib.Path:
    """A crate's crate-level documentation, wherever it lives.

    A library keeps it in `src/lib.rs`; `sipx-cli` is bin-only and keeps it in `src/main.rs`, and a
    reader that insisted on `lib.rs` would find no declaration there and report the crate as silent.
    """
    lib = CRATES / crate / "src" / "lib.rs"
    return lib if lib.exists() else CRATES / crate / "src" / "main.rs"


def crate_doc(crate: str) -> str:
    """Only the `//!` lines of a crate's entry point."""
    text = entry(crate).read_text(encoding="utf-8")
    return "\n".join(line for line in text.splitlines() if line.strip().startswith("//!"))


def classifies(crate: str, word: str) -> bool:
    """Whether a crate's own documentation claims any of itself is `word`.

    The glossary line that defines the word does not count as claiming it.

    The word is matched allowing punctuation *inside* the emphasis, because four of the ten crates
    write `**Supported.**` and the other four write `**Supported**`. A plain `**Supported**` substring
    test — which is what this had first — silently skipped `sipx-sip`, `sipx-rtp`, `sipx-audio` and
    `sipx-transport`, so four of the eight crates whose claims most need checking were not being
    checked at all. That is the failure direction that matters: a checker that reports nothing looks
    exactly like a codebase with nothing wrong.
    """
    glossary = _glossary(word)
    claim = re.compile(rf"\*\*{word}[^*]*\*\*")
    for line in crate_doc(crate).splitlines():
        body = line.strip().removeprefix("//!").strip()
        if body.startswith(glossary):
            continue
        if claim.search(body):
            return True
    return False


def declares_supported(crate: str) -> bool:
    """Whether a crate claims any of itself is `Supported`, outside the shared glossary."""
    return classifies(crate, "Supported")


def has_library(crate: str) -> bool:
    """Whether a crate can be depended on at all.

    `sipx-cli` is bin-only: it has no `src/lib.rs`, nothing is `pub`, and `cargo doc` renders it under
    the binary name. Asking whether an application reaches it is a category error — no application
    *can*, and `A-8` settled that its promise is its command-line surface, asserted by `tests/cli.rs`
    against `website/docs/reference/cli.md`. A Rust-API check has nothing true to say about it.
    """
    return (CRATES / crate / "src" / "lib.rs").exists()


def experimental_modules() -> dict[tuple[str, str], pathlib.Path]:
    """Every module marked `**Experimental** (`A-8`)`, by crate and module name.

    Keyed on the module's own file, so a module moved between `foo.rs` and `foo/mod.rs` is the same
    module to this checker.
    """
    found = {}
    for crate in published():
        for path, text in sources(crate).items():
            if not _MARKER.search(text):
                continue
            module = path.parent.name if path.name == "mod.rs" else path.stem
            found[(crate, module)] = path
    return found


def names_crate(text: str, target: str) -> bool:
    """Whether a source names another crate at all."""
    return f"{crate_path(target)}::" in text


def names_module(text: str, target: str, module: str) -> bool:
    """Whether a source names one module of another crate.

    Both the direct path and the `use` list are matched, because `use sipx_media::{bridge, session}`
    reaches `bridge` without ever writing `sipx_media::bridge`.
    """
    root = crate_path(target)
    if f"{root}::{module}" in text:
        return True
    for group in re.findall(rf"{root}::\{{(.*?)\}}", text, re.S):
        if module in re.split(r"[\s,]+", group):
            return True
    return False


def unreached_supported(reached: set[str]) -> list[str]:
    """Crates that claim `Supported` and that no path from an application arrives at."""
    problems = []
    for crate in published():
        if not has_library(crate) or not declares_supported(crate):
            continue
        if crate not in reached:
            problems.append(
                f"{crate} declares part of itself `Supported` and no application depends on it "
                f"— nothing in {' or '.join(APPLICATIONS)}'s dependency closure reaches it. Use it "
                f"from an application, or mark it `Experimental`: a supported surface with no "
                f"caller is the claim this check exists to refuse"
            )
            continue
        # A manifest edge is not use. Without this, adding a line to `Cargo.toml` would be enough
        # to launder a claim that no code backs.
        if crate in APPLICATIONS:
            continue
        callers = [
            other
            for other in sorted(reached)
            if other != crate
            and any(names_crate(text, crate) for text in sources(other).values())
        ]
        if not callers:
            problems.append(
                f"{crate} declares part of itself `Supported` and is depended on but never named: "
                f"no source in the closure writes `{crate_path(crate)}::`. A manifest entry is not "
                f"a caller"
            )
    return problems


#: A `pub mod` declaration and the feature gating it, if any.
_GATED_MODULE = re.compile(
    r'(?:#\[cfg\(feature = "(?P<feature>[\w-]+)"\)\]\s*\n\s*)?pub mod (?P<name>\w+);'
)


def module_gates(crate: str) -> dict[str, str]:
    """Each module of a crate, and the feature its declaration is gated by.

    Empty string when a module is unconditional. This is what tells the difference between a module
    the application could select and one it could not: `sipx_audio::opus` is declared behind
    `#[cfg(feature = "opus")]`, so no amount of naming it makes it reachable from a binary that does
    not enable the feature.
    """
    gates = {}
    for text in sources(crate).values():
        for match in _GATED_MODULE.finditer(text):
            gates.setdefault(match.group("name"), match.group("feature") or "")
    return gates


def selectable(crate: str, module: str, enabled: dict[str, set[str]]) -> bool:
    """Whether an application could turn this module on at all."""
    gate = module_gates(crate).get(module, "")
    return not gate or gate in enabled.get(crate, set())


def reached_experimental(enabled: dict[str, set[str]]) -> list[str]:
    """Experimental modules that something on a path from an application selects.

    A module behind a feature no application enables is skipped however loudly the closure names it.
    `sipx-media` writes `sipx_audio::opus` under `#[cfg(feature = "opus")]`; a checker that read the
    reference and not the gate would demand Opus graduate to `Supported` on the strength of code that
    no shipped binary compiles.
    """
    reached = set(enabled)
    problems = []
    for (crate, module), path in sorted(experimental_modules().items()):
        if not selectable(crate, module, enabled):
            continue
        for other in sorted(reached):
            if other == crate:
                continue
            for source, text in sources(other).items():
                if not names_module(text, crate, module):
                    continue
                problems.append(
                    f"{where(path)} marks `{crate_path(crate)}::{module}` "
                    f"**Experimental** and {where(source)} — reachable from "
                    f"{' or '.join(APPLICATIONS)} — selects it. A caller has now constrained its "
                    f"shape, so it graduates: move it to `Supported` with a CHANGELOG.md entry "
                    f"(README.md's stability rule). Do not stop the caller"
                )
                break
    return problems


def unselectable_and_unmarked(enabled: dict[str, set[str]]) -> list[str]:
    """Modules behind a feature no application enables, that do not say they are experimental.

    This is the direction the worked example needed. Opus (RFC 6716, 7587) is implemented, tested,
    selectable from `sipx-call` and compiled by every `--all-features` run — and behind
    `sipx-audio/opus`, which links libopus, is off by default, and which **no shipped binary can turn
    on**: `sipx-cli` takes no flag for it and declares no `[features]` table to forward one, and
    `sipx-app` deliberately does not enable it (see its `# Stability` section for why).

    A capability that is library-reachable and binary-unreachable is the exact shape alpha predicate 1
    is about, and the reason a path check could never settle it: every path is real. What settles it is
    that no application selects it. So the rule is symmetric with graduation — such a module must say
    it is experimental, and it stops having to the moment an application enables the feature.
    """
    problems = []
    marked = {(crate, module) for crate, module in experimental_modules()}
    for crate in sorted(enabled):
        if not has_library(crate):
            continue
        for module, gate in sorted(module_gates(crate).items()):
            if not gate or gate in enabled[crate]:
                continue
            if (crate, module) in marked:
                continue
            problems.append(
                f"`{crate_path(crate)}::{module}` is behind the `{gate}` feature, which no "
                f"application enables, and its module documentation does not mark it "
                f"**Experimental** (`A-8`). It is reachable from a library and from no shipped "
                f"binary, so no caller has constrained its shape — say so at the module, or enable "
                f"`{gate}` from an application and mean it"
            )
    return problems


def unreached(reached: set[str]) -> list[str]:
    """Published, dependable crates that no path from an application arrives at.

    These are the crates the surface does not cover, so each has to be declared `Experimental` — and
    together with the module markers they are the list assertion 3 requires to be non-empty. Bin-only
    crates are excluded for the reason `has_library` gives: nothing can depend on one, so its absence
    from the closure says nothing about anybody's claim.
    """
    return sorted(
        crate
        for crate in published()
        if has_library(crate) and crate not in reached and crate not in APPLICATIONS
    )


def report() -> tuple[list[str], set[str], dict[tuple[str, str], pathlib.Path]]:
    """Every disagreement between the declared surface and the application's use of it."""
    enabled = resolve(APPLICATIONS)
    reached = set(enabled)
    experimental = experimental_modules()
    problems = (
        unreached_supported(reached)
        + reached_experimental(enabled)
        + unselectable_and_unmarked(enabled)
    )

    unreached_crates = unreached(reached)
    if not experimental and not unreached_crates:
        problems.append(
            "nothing in the workspace is marked experimental and the application reaches every "
            "published crate; an application that needs the whole stack is a claim, and this one "
            "has stopped being checked"
        )
    return problems, reached, experimental


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] != "--check":
        print("usage: check-app-surface.py --check", file=sys.stderr)
        return 2

    problems, reached, experimental = report()
    if problems:
        print(
            "the declared surface and the application's own dependencies disagree:",
            file=sys.stderr,
        )
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    unreached_crates = unreached(reached)
    crates = "crate" if len(unreached_crates) == 1 else "crates"
    modules = "module" if len(experimental) == 1 else "modules"
    print(
        f"app surface: {' + '.join(APPLICATIONS)} reaches {len(reached)} of "
        f"{len(published())} published crates; {len(experimental)} {modules} and "
        f"{len(unreached_crates)} {crates} experimental "
        f"({', '.join(unreached_crates) or 'none'}), none of them selected from a call"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
