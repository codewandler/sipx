#!/usr/bin/env python3
"""Tests for `check-app-surface.py`, against fabricated workspaces with known answers.

The checker's whole product is a *judgement about claims*, so it is asserted on crates whose claims
are written down in the test rather than on the real tree. Running it against the real workspace and
reading the summary would prove only that it prints a summary.

Two of these are regressions for bugs this checker had while it was being written, and both failed in
the same direction — reporting nothing. A checker with no output looks exactly like a codebase with
nothing wrong, which makes that the only failure mode worth writing a test for twice:

- `the_shared_glossary_is_not_a_claim`: a multiline pattern whose `\\s` crossed a newline read every
  crate's definition of the two words as a claim to be one of them.
- `punctuation_inside_the_emphasis_is_still_a_claim`: four of the ten crates write `**Supported.**`
  and a substring test for `**Supported**` skipped all four.

`the_real_workspace_agrees` is the one test that reads the real tree, and it is the gate's subject
stated as an assertion so the suite fails on drift as well as the check does.
"""

import importlib.util
import pathlib
import shutil
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

_SPEC = importlib.util.spec_from_file_location(
    "app_surface", pathlib.Path(__file__).resolve().parent / "check-app-surface.py"
)
surface = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(surface)

#: The glossary every published crate carries, which defines the two words without claiming either.
GLOSSARY = """\
//! # Stability
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry.
//! - **Experimental** — may change shape or be removed without a migration note.
//!
"""


class Workspace:
    """A fabricated `crates/` tree, so a claim under test is one the test wrote."""

    def __init__(self):
        self.root = pathlib.Path(tempfile.mkdtemp())
        self.crates = self.root / "crates"
        self.crates.mkdir()
        #: The root `[workspace.dependencies]` table, by crate name. Written on every `add`, because
        #: this is a real input to the checker: an edge written `foo.workspace = true` inherits the
        #: features set here, and a fixture that had no root manifest would read the repository's own.
        self.workspace_dependencies: dict[str, str] = {}

    def inherits(self, name, table):
        """Set a root `[workspace.dependencies]` entry, e.g. `features = ["opus"]`."""
        self.workspace_dependencies[name] = table
        self._write_root()
        return self

    def _write_root(self):
        lines = ["[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\n"]
        for name, table in sorted(self.workspace_dependencies.items()):
            extra = f", {table}" if table else ""
            lines.append(f'{name} = {{ path = "crates/{name}"{extra} }}\n')
        (self.root / "Cargo.toml").write_text("".join(lines))

    def add(self, name, *, declares="", dependencies=(), dev_dependencies=(), modules=None,
            publish=True, library=True, features=None, gated=None, entry_extra=""):
        directory = self.crates / name
        (directory / "src").mkdir(parents=True)
        manifest = [f'[package]\nname = "{name}"\n']
        if not publish:
            manifest.append("publish = false\n")
        if features:
            manifest.append("\n[features]\n")
            for feature, activations in features.items():
                rendered = ", ".join(f'"{item}"' for item in activations)
                manifest.append(f"{feature} = [{rendered}]\n")
        manifest.append("\n[dependencies]\n")
        for dependency in dependencies:
            if isinstance(dependency, tuple):
                who, table = dependency
                manifest.append(f"{who} = {{ workspace = true, {table} }}\n")
            else:
                manifest.append(f"{dependency}.workspace = true\n")
        manifest.append("\n[dev-dependencies]\n")
        for dependency in dev_dependencies:
            manifest.append(f"{dependency}.workspace = true\n")
        (directory / "Cargo.toml").write_text("".join(manifest))

        entry = "lib.rs" if library else "main.rs"
        declarations = "".join(
            f'#[cfg(feature = "{gate}")]\npub mod {module};\n'
            for module, gate in (gated or {}).items()
        )
        (directory / "src" / entry).write_text(
            GLOSSARY + declares + "\n" + entry_extra + declarations
        )
        for module, text in (modules or {}).items():
            (directory / "src" / f"{module}.rs").write_text(text)
        for module in gated or {}:
            path = directory / "src" / f"{module}.rs"
            if not path.exists():
                path.write_text("//! A gated module.\n")

        self.workspace_dependencies.setdefault(name, "")
        self._write_root()
        return self

    def __enter__(self):
        self._saved = (surface.CRATES, surface.ROOT)
        surface.CRATES = self.crates
        surface.ROOT = self.root
        self._write_root()
        return self

    def __exit__(self, *_):
        surface.CRATES, surface.ROOT = self._saved
        shutil.rmtree(self.root, ignore_errors=True)
        return False


class TheClassification(unittest.TestCase):
    def test_the_shared_glossary_is_not_a_claim(self):
        """A crate that only *defines* the two words claims neither.

        The regression: a pattern that began at the blank `//!` line above the glossary and stepped
        over it read `sipx-app-protocol` — which declares itself wholly experimental — as claiming
        supported surface.
        """
        with Workspace() as workspace:
            workspace.add("sipx-quiet")
            self.assertFalse(
                surface.declares_supported("sipx-quiet"),
                "the line defining `Supported` must not count as claiming it",
            )
            self.assertFalse(surface.classifies("sipx-quiet", "Experimental"))

    def test_punctuation_inside_the_emphasis_is_still_a_claim(self):
        """`**Supported.**` and `**Supported**` are the same claim.

        The regression: four of the ten real crates write the first form, and a substring test for
        the second skipped every one of them.
        """
        with Workspace() as workspace:
            workspace.add("sipx-terse", declares="//! **Supported.** All of it.")
            workspace.add("sipx-plain", declares="//! **Supported** — all of it.")
            self.assertTrue(surface.declares_supported("sipx-terse"))
            self.assertTrue(surface.declares_supported("sipx-plain"))

    def test_a_crate_claiming_nothing_is_not_read_as_supported(self):
        with Workspace() as workspace:
            workspace.add("sipx-wholly", declares="//! **Experimental.** Nothing selects it.")
            self.assertFalse(surface.declares_supported("sipx-wholly"))
            self.assertTrue(surface.classifies("sipx-wholly", "Experimental"))


class TheClosure(unittest.TestCase):
    def test_dependencies_are_followed_transitively(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", dependencies=["sipx-mid"])
            workspace.add("sipx-mid", dependencies=["sipx-deep"])
            workspace.add("sipx-deep")
            self.assertEqual(
                surface.closure(("sipx-app",)),
                {"sipx-app", "sipx-mid", "sipx-deep"},
                "a crate reached through another is reached",
            )

    def test_a_dev_dependency_is_not_a_caller(self):
        """The exclusion the whole predicate rests on: a test is not a caller.

        If dev-dependencies counted, the test suite would place every crate it exercises on the
        supported surface — which is exactly why the suite could not settle this predicate itself.
        """
        with Workspace() as workspace:
            workspace.add("sipx-app", dev_dependencies=["sipx-fixture"])
            workspace.add("sipx-fixture")
            self.assertEqual(surface.closure(("sipx-app",)), {"sipx-app"})

    def test_a_crate_that_does_not_publish_is_still_a_step_on_the_path(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", dependencies=["sipx-private"])
            workspace.add("sipx-private", publish=False)
            self.assertIn("sipx-private", surface.closure(("sipx-app",)))
            self.assertNotIn("sipx-private", surface.published())


class TheAssertions(unittest.TestCase):
    def test_a_supported_crate_no_application_reaches_is_reported(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add("sipx-orphan", declares="//! **Supported.** All of it.")
            problems = surface.unreached_supported(surface.closure(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("sipx-orphan", problems[0])

    def test_a_manifest_edge_is_not_use(self):
        """Depending on a crate without naming it must not launder its claim.

        Otherwise the cheapest way to make this check pass would be to add a line to `Cargo.toml`,
        which would make the surface a property of the manifest rather than of the program.
        """
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-unused"],
            )
            workspace.add("sipx-unused", declares="//! **Supported.** All of it.")
            problems = surface.unused_edges(surface.closure(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("never named", problems[0])
            self.assertIn("also declares part of itself `Supported`", problems[0])

    def test_a_manifest_edge_is_not_use_whatever_the_crate_claims(self):
        """The generalised form: the laundering route does not depend on claiming `Supported`.

        Reporting only the `Supported` case left the count movable from a manifest. Adding
        `sipx-app-protocol` — which claims nothing — to the host's manifest put it in the closure and
        silently dropped the experimental-crate list from one entry to none.
        """
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-quiet"],
            )
            workspace.add("sipx-quiet", declares="//! # Experimental\n//!\n//! Says nothing else.")
            problems = surface.unused_edges(surface.closure(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("never named", problems[0])
            self.assertNotIn("Supported", problems[0])

    def test_naming_the_crate_in_code_satisfies_it(self):
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-used"],
                modules={"host": "use sipx_used::thing;\n"},
            )
            workspace.add("sipx-used", declares="//! **Supported.** All of it.")
            self.assertEqual(surface.unreached_supported(surface.closure(("sipx-app",))), [])

    def test_an_experimental_module_the_application_selects_is_reported(self):
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::secret::Thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={"secret": "//! **Experimental** (`A-8`): nothing above selects it.\n"},
            )
            problems = surface.reached_experimental(surface.resolve(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("graduates", problems[0])

    def test_a_use_list_selects_a_module_without_naming_its_path(self):
        """`use sipx_deep::{secret, other}` reaches `secret` without writing `sipx_deep::secret`."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::{other, secret};\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={
                    "secret": "//! **Experimental** (`A-8`): nothing above selects it.\n",
                    "other": "//! Fine.\n",
                },
            )
            problems = surface.reached_experimental(surface.resolve(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)

    def test_a_module_its_own_crate_uses_is_not_a_caller_above_it(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={
                    "secret": "//! **Experimental** (`A-8`): nothing above selects it.\n",
                    "inside": "use sipx_deep::secret::Thing;\n",
                },
            )
            self.assertEqual(surface.reached_experimental(surface.resolve(("sipx-app",))), [])

    def test_a_test_module_is_not_a_caller(self):
        """A reference below `#[cfg(test)]` is a test, and a test does not widen the surface."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "#[cfg(test)]\nuse sipx_deep::secret::Thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={"secret": "//! **Experimental** (`A-8`): nothing selects it.\n"},
            )
            self.assertEqual(surface.reached_experimental(surface.resolve(("sipx-app",))), [])

    def test_a_bin_only_crate_is_not_judged(self):
        """Nothing can depend on a crate with no library, so its absence accuses nobody."""
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-tool",
                declares="//! **Supported**: its commands and flags.",
                library=False,
            )
            self.assertFalse(surface.has_library("sipx-tool"))
            self.assertEqual(surface.unreached_supported(surface.closure(("sipx-app",))), [])


class FeaturesArePartOfSelection(unittest.TestCase):
    """`X-38`'s worked example: a capability reachable from the library and from no binary.

    The failure this prevents is an over-claim, and it is the subtlest one in the story. Opus is
    implemented, tested, cited against RFC 6716 and 7587, selectable from `sipx-call`, and compiled by
    every `--all-features` run — and no shipped binary can turn it on. A checker that walked dependency
    names, or that read `--all-features` as "what is reachable", would call it supported.
    """

    def test_an_optional_dependency_no_feature_enables_is_not_reached(self):
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=[("sipx-extra", "optional = true")],
            )
            workspace.add("sipx-extra", declares="//! **Supported.** All of it.")
            self.assertNotIn("sipx-extra", surface.closure(("sipx-app",)))

    def test_a_feature_the_root_enables_carries_its_dependency_in(self):
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                features={"default": ["codec"], "codec": ["dep:sipx-extra"]},
                dependencies=[("sipx-extra", "optional = true")],
                modules={"host": "use sipx_extra::thing;\n"},
            )
            workspace.add("sipx-extra", declares="//! **Supported.** All of it.")
            self.assertIn("sipx-extra", surface.closure(("sipx-app",)))

    def test_a_feature_is_propagated_to_a_dependency(self):
        """`sipx-media/opus = ["sipx-audio/opus"]` is this shape, and it has to reach two levels."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                features={"default": ["codec"], "codec": ["sipx-mid/codec"]},
                dependencies=["sipx-mid"],
                modules={"host": "use sipx_mid::thing;\n"},
            )
            workspace.add(
                "sipx-mid",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": ["sipx-deep/codec"]},
                dependencies=["sipx-deep"],
                modules={"inner": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
            )
            enabled = surface.resolve(("sipx-app",))
            self.assertIn("codec", enabled["sipx-mid"])
            self.assertIn(
                "codec",
                enabled["sipx-deep"],
                "a feature asked for through an intermediate crate must arrive",
            )

    def test_a_module_behind_an_unenabled_feature_must_say_it_is_experimental(self):
        """The Opus case exactly: gated, unenabled, and silent about it."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                gated={"fancy": "codec"},
                modules={"fancy": "//! A codec, and nothing says it is experimental.\n"},
            )
            problems = surface.unselectable_and_unmarked(surface.resolve(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("sipx_deep::fancy", problems[0])
            self.assertIn("`codec`", problems[0])

    def test_marking_it_experimental_satisfies_the_rule(self):
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                gated={"fancy": "codec"},
                modules={"fancy": "//! **Experimental** (`A-8`): nothing enables `codec`.\n"},
            )
            self.assertEqual(
                surface.unselectable_and_unmarked(surface.resolve(("sipx-app",))), []
            )

    def test_a_gated_module_the_application_enables_is_not_asked_to_be_experimental(self):
        """The demotion rule's mirror: enable the feature and the capability joins the surface."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                features={"default": ["codec"], "codec": ["sipx-deep/codec"]},
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                gated={"fancy": "codec"},
                modules={"fancy": "//! A codec, and nothing says it is experimental.\n"},
            )
            self.assertEqual(
                surface.unselectable_and_unmarked(surface.resolve(("sipx-app",))),
                [],
                "an enabled feature puts the module on the surface, so silence is correct",
            )

    def test_an_experimental_module_nothing_can_select_is_not_asked_to_graduate(self):
        """A reference under a feature gate must not count as a caller.

        `sipx-media` writes `sipx_audio::opus` under `#[cfg(feature = "opus")]`. Reading the reference
        and not the gate would demand Opus graduate on the strength of code no binary compiles.
        """
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-mid"],
                modules={"host": "use sipx_mid::thing;\n"},
            )
            workspace.add(
                "sipx-mid",
                declares="//! **Supported.** All of it.",
                dependencies=["sipx-deep"],
                modules={"inner": "use sipx_deep::fancy::Thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                gated={"fancy": "codec"},
                modules={"fancy": "//! **Experimental** (`A-8`): nothing enables `codec`.\n"},
            )
            self.assertEqual(
                surface.reached_experimental(surface.resolve(("sipx-app",))),
                [],
                "a caller behind an unenabled feature gate is not a caller",
            )


class TheReviewFindings(unittest.TestCase):
    """The four attacks that got through the first version, each of which passed *silently*.

    Kept as one class because they share a shape: every one of them let a one-line edit move the
    reported surface while the checker printed success. That is the failure direction this script's own
    docstrings call the worst case, so each gets a test naming the edit that used to work.
    """

    def test_a_feature_set_in_the_workspace_table_is_seen(self):
        """`B1`: every crate writes `foo.workspace = true`, so this is where a feature goes."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                gated={"fancy": "codec"},
                modules={"fancy": "//! **Experimental** (`A-8`): nothing enables `codec`.\n"},
            )
            self.assertNotIn("codec", surface.resolve(("sipx-app",))["sipx-deep"])

            workspace.inherits("sipx-deep", 'features = ["codec"]')
            self.assertIn(
                "codec",
                surface.resolve(("sipx-app",))["sipx-deep"],
                "a feature set in the root table genuinely ships, so the checker must see it",
            )

    def test_a_per_crate_feature_list_adds_to_the_inherited_one(self):
        """Cargo's rule, and the reason both tables have to be read rather than either one."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=[("sipx-deep", 'features = ["extra"]')],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": [], "extra": []},
            )
            workspace.inherits("sipx-deep", 'features = ["codec"]')
            enabled = surface.resolve(("sipx-app",))["sipx-deep"]
            self.assertIn("codec", enabled, "the inherited feature survives")
            self.assertIn("extra", enabled, "and the per-crate one is added, not substituted")

    def test_a_wholly_experimental_crate_the_application_selects_is_reported(self):
        """`B2`: `README.md` promised this and only the module-sized case was checked."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-contract"],
                modules={"host": "use sipx_contract::Envelope;\n"},
            )
            workspace.add(
                "sipx-contract",
                declares="//! # Experimental\n//!\n//! It settles when two applications exist.",
            )
            self.assertEqual(surface.wholly_experimental(), ["sipx-contract"])
            problems = surface.reached_experimental_crates(surface.resolve(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("declares its whole self **Experimental**", problems[0])
            self.assertIn("graduates", problems[0])

    def test_listing_which_modules_are_experimental_is_not_a_whole_crate_claim(self):
        """Six library crates write `**Experimental**` while listing their modules."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares=(
                    "//! **Supported**: the parser.\n"
                    "//! **Experimental**: `presence` and `subscribe`.\n"
                ),
            )
            self.assertEqual(
                surface.wholly_experimental(),
                [],
                "naming which modules are experimental is not declaring the crate so",
            )

    def test_a_comment_is_not_a_caller(self):
        """`B4`: the hole `M-30` closed in `rfc-report.py`, in this checker."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-contract"],
                modules={
                    "host": "//! One day we might use sipx_contract::Envelope here.\n",
                },
            )
            workspace.add("sipx-contract", declares="//! # Experimental\n//!\n//! Nothing yet.")
            self.assertEqual(
                surface.reached_experimental_crates(surface.resolve(("sipx-app",))),
                [],
                "a sentence about a symbol calls nothing",
            )
            problems = surface.unused_edges(surface.closure(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("never named", problems[0])

    def test_a_comment_does_not_demand_a_graduation_either(self):
        """It fired both ways: prose naming an experimental module raised a spurious demand."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={
                    "host": "use sipx_deep::thing;\n// see also sipx_deep::fancy one day\n",
                },
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={"fancy": "//! **Experimental** (`A-8`): nothing selects it.\n"},
                entry_extra="pub mod fancy;\n",
            )
            self.assertEqual(
                surface.reached_experimental(surface.resolve(("sipx-app",))),
                [],
                "a comment must not force a module to graduate",
            )

    def test_a_compound_cfg_still_gates_a_module(self):
        """`B5`: `all(feature = "opus", not(doc))` read as unconditional."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-deep"],
                modules={"host": "use sipx_deep::thing;\n"},
            )
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"default": [], "codec": []},
                modules={"fancy": "//! A codec that says nothing about its status.\n"},
                entry_extra='#[cfg(all(feature = "codec", not(doc)))]\npub mod fancy;\n',
            )
            gate = surface.module_gates("sipx-deep")["fancy"]
            self.assertEqual(gate.features, frozenset({"codec"}))
            self.assertFalse(gate.satisfied_by(set()), "an unenabled feature still gates it")
            problems = surface.unselectable_and_unmarked(surface.resolve(("sipx-app",)))
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("sipx_deep::fancy", problems[0])

    def test_any_is_read_as_a_disjunction(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"a": [], "b": []},
                modules={"fancy": "//! Something.\n"},
                entry_extra='#[cfg(any(feature = "a", feature = "b"))]\npub mod fancy;\n',
            )
            gate = surface.module_gates("sipx-deep")["fancy"]
            self.assertTrue(gate.satisfied_by({"b"}), "either feature is enough")
            self.assertFalse(gate.satisfied_by(set()))

    def test_a_non_feature_atom_does_not_gate_anything(self):
        """`not(doc)` says nothing about which features a module needs."""
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                modules={"fancy": "//! Something.\n"},
                entry_extra="#[cfg(not(doc))]\npub mod fancy;\n",
            )
            self.assertTrue(surface.module_gates("sipx-deep")["fancy"].satisfied_by(set()))

    def test_two_modules_of_the_same_name_under_different_parents_are_distinct(self):
        """The bare-name key collided, and `setdefault` kept whichever was read first."""
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-deep",
                declares="//! **Supported.** All of it.",
                features={"one": [], "two": []},
            )
            source = surface.CRATES / "sipx-deep" / "src"
            for parent, feature in (("first", "one"), ("second", "two")):
                (source / parent).mkdir()
                (source / parent / "mod.rs").write_text(
                    f'//! A parent.\n#[cfg(feature = "{feature}")]\npub mod shared;\n'
                )
                (source / parent / "shared.rs").write_text("//! A leaf.\n")
            (source / "lib.rs").write_text(
                (source / "lib.rs").read_text() + "pub mod first;\npub mod second;\n"
            )
            gates = surface.module_gates("sipx-deep")
            self.assertEqual(gates["first::shared"].features, frozenset({"one"}))
            self.assertEqual(gates["second::shared"].features, frozenset({"two"}))

    def test_a_claim_citing_the_cli_must_be_a_claim_the_cli_backs(self):
        """`B3`: the largest `Supported` claim in the tree rests on a citation nothing verified.

        `sipx-ua`'s declaration justifies registration, digest auth, Path, Service-Route, Outbound and
        push by `sipx register --outbound` — by the CLI, which `APPLICATIONS` refuses to count. That is
        a real, shipped caller, so the claim is not demoted; what was missing is that the citation was
        trusted. Now it is checked.
        """
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-cli",
                declares="//! The command line.",
                library=False,
                modules={"main": "fn main() {}\n"},
            )
            workspace.add(
                "sipx-registrar",
                declares="//! **Supported**: leases, driven by `sipx register`.",
            )
            problems = surface.cli_cited_but_uncalled()
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("cites the command line", problems[0])

    def test_a_citation_the_cli_really_backs_passes(self):
        with Workspace() as workspace:
            workspace.add("sipx-app", declares="//! **Experimental.**")
            workspace.add(
                "sipx-cli",
                declares="//! The command line.",
                library=False,
                modules={"main": "use sipx_registrar::Lease;\nfn main() {}\n"},
            )
            workspace.add(
                "sipx-registrar",
                declares="//! **Supported**: leases, driven by `sipx register`.",
            )
            self.assertEqual(surface.cli_cited_but_uncalled(), [])

    def test_a_claim_citing_no_application_is_not_asked_about_the_cli(self):
        """Only a declaration that names the CLI is held to naming it truthfully."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Experimental.**",
                dependencies=["sipx-quiet"],
                modules={"host": "use sipx_quiet::thing;\n"},
            )
            workspace.add(
                "sipx-cli",
                declares="//! The command line.",
                library=False,
                modules={"main": "fn main() {}\n"},
            )
            workspace.add("sipx-quiet", declares="//! **Supported**: the parser.")
            self.assertEqual(surface.cli_cited_but_uncalled(), [])

    def test_a_test_module_in_the_middle_of_a_file_does_not_blind_the_rest(self):
        """The minor: truncating at the first `#[cfg(test)]` discarded 30.2% of `crates/*/src`."""
        text = (
            "use sipx_deep::real;\n"
            "#[cfg(test)]\nmod tests {\n    use sipx_deep::fake;\n    fn f() { let _ = 1; }\n}\n"
            "use sipx_deep::also_real;\n"
        )
        cut = surface.code(text)
        self.assertIn("also_real", cut, "the code after a test module is still code")
        self.assertIn("real", cut)
        self.assertNotIn("fake", cut, "and the test module itself is still cut")


class TheNonEmptyRule(unittest.TestCase):
    def test_an_application_that_needs_everything_is_reported(self):
        """Acceptance item 4: a shipped app that needs the whole stack is a claim, not a result."""
        with Workspace() as workspace:
            workspace.add(
                "sipx-app",
                declares="//! **Supported.**",
                dependencies=["sipx-all"],
                modules={"host": "use sipx_all::thing;\n"},
            )
            workspace.add("sipx-all", declares="//! **Supported.** All of it.")
            saved = surface.APPLICATIONS
            surface.APPLICATIONS = ("sipx-app",)
            try:
                problems, _, experimental = surface.report()
            finally:
                surface.APPLICATIONS = saved
            self.assertEqual(experimental, {})
            self.assertTrue(
                any("needs the whole stack" in problem for problem in problems),
                problems,
            )


class TheRealWorkspace(unittest.TestCase):
    def test_the_real_workspace_agrees(self):
        """`--check`'s subject, asserted here so the suite fails on drift as well as the check."""
        problems, _, _ = surface.report()
        self.assertEqual(problems, [], "run ./scripts/check-app-surface.py --check")

    def test_the_experimental_list_is_not_empty(self):
        """The list `X-38` requires to be non-empty, on the real tree rather than a fixture."""
        _, reached, experimental = surface.report()
        self.assertTrue(
            experimental or surface.unreached(reached),
            "no module or unreached published crate remains experimental",
        )

    def test_opus_is_experimental_because_no_binary_can_select_it(self):
        """`X-38`'s worked example, pinned against the real tree.

        RFC 6716 and 7587 are implemented, tested and selectable from `sipx-call`, and Opus is behind
        `sipx-audio/opus`, which links libopus. No shipped binary enables it: `sipx-cli` has no flag
        and no `[features]` table, and `sipx-app` deliberately does not. So it is *Experimental* under
        this story's definition, and this fails if someone enables the feature from the host without
        moving the declaration, or removes the declaration without enabling it.
        """
        enabled = surface.resolve(surface.APPLICATIONS)
        self.assertIn("sipx-audio", enabled, "the host reaches the codec crate")
        self.assertNotIn(
            "opus",
            enabled["sipx-audio"],
            "no application enables `opus`; if one now does, Opus graduates and this must change",
        )
        self.assertIn(
            ("sipx-audio", "opus"),
            surface.experimental_modules(),
            "a capability no binary can select has to say so at the module",
        )

    def test_the_application_is_neither_the_test_suite_nor_the_cli(self):
        """Acceptance item 1, as an assertion: the root is a program, and not the one we had."""
        self.assertIn("sipx-app", surface.APPLICATIONS)
        self.assertNotIn("sipx-cli", surface.APPLICATIONS)
        self.assertTrue(
            (surface.CRATES / "sipx-app" / "src" / "host.rs").exists(),
            "the application that defines the surface has to exist",
        )

    def test_something_actually_runs_the_application(self):
        """**Existing is not running** (`X-38` rework).

        This assertion checked that `host.rs` was on disk, which the review pointed out is the whole of
        what it checked: `serve`, `admit`, `carry`, `answer_out_of_dialog` and `refuse` were executed by
        nothing, and the commit message said "sipx-app answers a call". A surface defined by an
        application nobody runs rests on what compiles — the same weakness as the path checks this
        predicate replaced, since `README.md`'s claim is that an application "has no dead branch to
        cite".

        So the acceptance is now that an integration test exists, drives the host over a socket, and
        covers both branches of the knob the host routes an unreachable app through. Asserted on the
        file's content rather than on a name, because a test that was renamed and gutted would still
        satisfy a name.
        """
        integration = surface.CRATES / "sipx-app" / "tests" / "host.rs"
        self.assertTrue(
            integration.exists(),
            "the application that defines the surface has to be run by something",
        )
        text = integration.read_text(encoding="utf-8")
        for needle, why in (
            ("Host::start", "the test has to start the real host"),
            ("host.serve(", "and drive its real loop"),
            ("dial(", "with a real invitation over a real socket"),
            ("Method::Options", "covering the out-of-dialog branch (RFC 3261 §11)"),
            ("on_failure", "and both branches of the document's failure knob"),
        ):
            self.assertIn(needle, text, why)


if __name__ == "__main__":
    unittest.main()
