#!/usr/bin/env python3
"""Tests for rfc-report.py's schema guard.

The registry's grain is per RFC, decided in `docs/designs/rfc-registry-grain.md`. A decision
that lives only in prose is a convention, and conventions rot: the way this one rots is that
somebody adds a finer-grained row, `tomllib` parses it happily, and the checker walks straight
past it — so the claim is in the source, absent from the generated table, and nobody is told.

These tests hold the guard that makes the decision enforceable rather than merely stated.
"""

import importlib.util
import pathlib
import re
import sys
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_report_module():
    """Import rfc-report.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "rfc_report", ROOT / "scripts" / "rfc-report.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


report = load_report_module()


def registry_entries():
    return tomllib.loads((ROOT / "docs" / "rfc" / "registry.toml").read_text())["rfc"]


def an_entry(**overrides):
    """A minimal well-formed entry, so a test can vary exactly one thing about it."""
    entry = {
        "number": 9999,
        "title": "A tracked document",
        "layer": "wire",
        "status": "none",
        "evidence": [],
        "note": "",
    }
    entry.update(overrides)
    return entry


class SchemaGuard(unittest.TestCase):
    """An entry may carry the keys the schema names, and no others."""

    def test_requirement_grain_row_is_rejected(self):
        """`[[rfc.requirement]]` must fail loudly, not be silently dropped.

        This is the decision under test. X-15 declined requirement-grain rows; the failure mode
        that decision has to survive is a row added anyway — by a contributor, or by a downstream
        registry merging its own extension back. `tomllib` turns `[[rfc.requirement]]` into a
        `requirement` key on the entry, so the guard sees it as an unknown key.
        """
        entry = an_entry(requirement=[{"section": "4.4.2", "status": "implemented"}])
        problems = report.schema_problems(entry)
        self.assertTrue(
            any("requirement" in p for p in problems),
            f"a requirement-grain row was accepted in silence; problems={problems}",
        )

    def test_requirement_row_is_rejected_through_the_public_check(self):
        """The guard has to fire from `--check`, which is the only entry point CI runs."""
        entries = registry_entries()
        entries.append(an_entry(number=9999, requirement=[{"section": "1"}]))
        problems = report.check(entries)
        self.assertTrue(
            any("requirement" in p and "9999" in p for p in problems),
            f"check() did not report the requirement row; problems={problems}",
        )

    def test_misspelled_key_is_rejected(self):
        """`role` is not `roles`, and the Roles column is load-bearing.

        The generated table renders a missing `roles` as an em dash. A typo therefore understates
        which roles sipx implements — the one direction of error the table must not make silently,
        since the whole point of the column is that a UA implementation does not imply the proxy
        half.
        """
        problems = report.schema_problems(an_entry(role=["uac"]))
        self.assertTrue(
            any("role" in p for p in problems),
            f"a misspelled key was accepted; problems={problems}",
        )

    def test_missing_required_key_is_rejected(self):
        entry = an_entry()
        del entry["note"]
        problems = report.schema_problems(entry)
        self.assertTrue(
            any("note" in p for p in problems),
            f"a missing required key was accepted; problems={problems}",
        )

    def test_a_well_formed_entry_is_accepted(self):
        """The guard must not cost a legitimate entry anything."""
        entry = an_entry(
            status="implemented",
            evidence=["scripts/rfc-report.py"],
            roles=["uac", "uas"],
            headers=["Via"],
            methods=["INVITE"],
            note="A note.",
        )
        self.assertEqual([], report.schema_problems(entry))


class SpecCitation(unittest.TestCase):
    """`spec` names the normative document for an RFC, and must name one that exists.

    The field is the registry's half of AGENTS.md non-negotiable 4. A spec no row points at is a
    spec nobody arrives at from the compliance table, which is the document a reader reaches for
    when asking "what does sipx do about RFC 3711" — and a `spec` naming a file that has since been
    moved is worse than none, because the table still reads as though the subsystem is specified.
    Held to the same standard as `evidence`, for the same reason.
    """

    def test_spec_is_an_accepted_key(self):
        self.assertEqual([], report.schema_problems(an_entry(spec="docs/specs/srtp.md")))

    def test_spec_must_be_a_string(self):
        problems = report.schema_problems(an_entry(spec=["docs/specs/srtp.md"]))
        self.assertTrue(
            any("spec" in p for p in problems),
            f"a list-valued spec was accepted; problems={problems}",
        )

    def test_a_spec_that_does_not_exist_is_reported(self):
        problems = report.check([an_entry(number=9999, spec="docs/specs/no-such-spec.md")])
        self.assertTrue(
            any("no-such-spec.md" in p for p in problems),
            f"a dangling spec citation was accepted; problems={problems}",
        )

    def test_the_spec_reaches_the_generated_table(self):
        """A claim that never leaves the source is the failure this checker exists to prevent.

        The link is relative to `docs/compliance.md`, which is where it is clicked — not the
        repository-relative path the registry stores, which would be dead everywhere but the root.
        """
        entries = [an_entry(number=9999, layer="media", spec="docs/specs/srtp.md")]
        self.assertIn("[srtp](specs/srtp.md)", report.render(entries))
        self.assertNotIn("(docs/specs/srtp.md)", report.render(entries))

    def test_the_srtp_family_cites_its_spec(self):
        """M-25's acceptance, in executable form.

        SRTP and its two keyings shipped without a spec (`M-14`, `M-15`); `X-25` found the breach
        and `M-25` closed it. These five rows are how a reader of the compliance table finds the
        document, so the citation is asserted rather than left to survive the next edit.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (3711, 4568, 5763, 5764, 8122):
            with self.subTest(rfc=number):
                self.assertEqual(by_number[number].get("spec"), "docs/specs/srtp.md")


class RoleReachability(unittest.TestCase):
    """A media row may not claim a UA role no call can select.

    The registry's one failure mode, five times in two days: a capability is built in a crate,
    the row claims `uac` and `uas`, and nothing above the crate ever calls it — so the compliance
    table reports as shipped something an application cannot ask for. Every existing check passes
    for such a row, because the header is known, the file exists, and evidence was cited.

    `sipx-call` is where an application asks for a call, so a media capability whose evidence
    never reaches it is one no role can perform. See `docs/designs/rfc-registry-grain.md` for why
    the rule is scoped to the media layer rather than applied to every row.
    """

    def a_media_entry(self, **overrides):
        entry = an_entry(
            number=9999,
            layer="media",
            status="implemented",
            roles=["uac", "uas"],
            note="A note.",
        )
        entry.update(overrides)
        return entry

    def test_a_media_role_claimed_from_a_leaf_crate_is_rejected(self):
        """The failing-first case: both roles claimed, evidence only in the crate below.

        This is `M-15`'s DTLS-SRTP row and `M-22`'s ICE rows in fixture form — code that exists,
        is tested by its own crate, and that no call has ever reached.
        """
        entry = self.a_media_entry(
            evidence=[
                "crates/sipx-media/src/dtls/mod.rs",
                "crates/sipx-media/tests/dtls_srtp.rs",
            ]
        )
        problems = report.check([entry])
        self.assertTrue(
            any("9999" in p and "reach" in p for p in problems),
            f"a media role no call can select was accepted; problems={problems}",
        )

    def test_evidence_at_the_call_layer_satisfies_the_rule(self):
        """`M-29`'s fix to RFC 4568, in fixture form: a call-level test is what makes it true."""
        entry = self.a_media_entry(
            evidence=[
                "crates/sipx-media/src/dtls/mod.rs",
                "crates/sipx-call/tests/secure_media.rs",
            ]
        )
        self.assertEqual([], [p for p in report.check([entry]) if "reach" in p])

    def test_a_row_claiming_no_role_is_not_asked_to_reach_a_call(self):
        """`M-28`'s fix was to *remove* `roles`, which must be a way through the check.

        RFC 5763 and 5764 state the gap in prose and claim no role. That is an honest row, and a
        check that still rejected it would push the next author towards a caveat nobody reads.
        """
        entry = self.a_media_entry(
            roles=[], status="partial", evidence=["crates/sipx-media/src/dtls/mod.rs"]
        )
        self.assertEqual([], [p for p in report.check([entry]) if "reach" in p])

    def test_the_rule_is_scoped_to_the_media_layer(self):
        """Deliberately narrow, and asserted so that widening it is a decision rather than a slip.

        The scope is a choice. Media is where a capability has to be *selected* before a call can
        use it, and selecting nothing is the silent default. A security row like RFC 2617 is
        reachable through `sipx-cli`, which depends on `sipx-ua` *and* `sipx-call`; it is out of
        scope because the check would be asking a question that cannot come out `no`, not because
        it could not answer it. `docs/designs/rfc-registry-grain.md` carries the count and the
        argument.

        **This test is also the dodge.** It relabels an otherwise-rejected media row `security` and
        asserts that it passes — which is exactly what an author wanting out of the check would do,
        since nothing validates `layer` beyond membership of `LAYER_TITLE`. That is the honest cost
        of scoping by an author-set field, it is the second entry in the design's "what would widen
        this", and it is written here rather than only in prose so the escape is visible to anyone
        reading the check's own tests.
        """
        entry = self.a_media_entry(
            layer="security", evidence=["crates/sipx-ua/src/auth.rs"]
        )
        self.assertEqual([], [p for p in report.check([entry]) if "reach" in p])

    def test_the_services_rows_keep_their_roles_and_why(self):
        """RFC 3680, 3856, 3903 and 4235 are *not* the media over-claims one layer over.

        They implement a `uas` surface in `sipx-ua` that nothing in `sipx-cli` calls, which is the
        ICE shape until you ask *which crate serves the claimed role*. For a media row that is
        `sipx-call`, a different crate sitting above the one that implements the capability, and
        something there has to select it. For a services row it is `sipx-ua` itself: the notifier
        is `sipx_ua::subscribe::Subscriptions`, no crate above it must select anything, and
        `sipx-call` does not depend on `sipx-ua` at all — so asking these rows to cite the call
        layer would ask them to cite a crate that does not and should not depend on them.

        That dependency direction is the load-bearing fact and is asserted here. `packages.rs` is
        asserted too, but only for what it shows — the surface being driven from outside its crate
        rather than merely compiled. It is *not* what distinguishes these rows from ICE:
        `crates/sipx-media/tests/ice.rs` calls `start_with_ice` from outside `sipx-media` in
        exactly the same way, and an earlier version of this docstring claimed otherwise.

        `docs/designs/rfc-registry-grain.md` carries the argument and both corrections.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (3680, 3856, 3903, 4235):
            with self.subTest(rfc=number):
                self.assertTrue(by_number[number].get("roles"))

        # The manifest fact: `sipx-ua` is a sibling of the call layer, not a crate below it, so no
        # call-layer citation could exist for a capability it serves.
        self.assertNotIn("sipx-ua", report.call_layer_crates())
        call_manifest = tomllib.loads(
            (ROOT / "crates" / "sipx-call" / "Cargo.toml").read_text()
        )
        self.assertNotIn("sipx-ua", call_manifest.get("dependencies", {}))
        # Whereas the media crates *are* below it, which is what makes selection a real question
        # there and not here.
        for crate in ("sipx-media", "sipx-sdp"):
            with self.subTest(crate=crate):
                self.assertIn(crate, call_manifest["dependencies"])

        reached = (ROOT / "crates" / "sipx-ua" / "tests" / "packages.rs").read_text()
        self.assertIn("sipx_ua::presence", reached)
        self.assertIn("sipx_ua::packages", reached)
        self.assertIn("sipx_ua::subscribe::{Answer, Subscriptions}", reached)
        # And driven, not just imported: a real SUBSCRIBE goes into the notifier.
        self.assertIn("notifier.on_subscribe(", reached)

    def test_the_scope_tracks_selection_not_the_layer_string(self):
        """The property behind `ROLE_REACHABILITY_LAYERS`, held against the code.

        A media capability is *selected*: it is carried only because something asked for it, and
        asking for nothing is the silent default. That — not the layer string — is why media rows
        are the ones checked. `layer` is a proxy, so this test holds the two in agreement on the
        media rows that claim a role, and a registry where they part company fails the gate instead
        of reading as measured.

        Scope of the agreement, stated because it is narrower than it sounds: rows without `roles`
        are outside the check entirely, so RFC 6716 and 7587 claim `implemented` for Opus, which no
        call can select, and nothing here objects. That limit is `X-33` and is recorded in
        `docs/designs/rfc-registry-grain.md` under "the gate is on `roles`, not on `status`".

        Both halves matter. The media rows that keep their roles must have a selector a call
        actually runs; the media rows whose roles `X-30` removed must not. The second half is the
        evidence that 8445 and 8839 were genuine over-claims and not the rule misfiring.
        """
        call_src = (ROOT / "crates" / "sipx-call" / "src").rglob("*.rs")
        call_source = "\n".join(f.read_text() for f in call_src)

        # RFC 3711 and 4568 keep `uac, uas`: SDES/SRTP is selected, and the selection has callers
        # in the crate that serves the role.
        self.assertIn(
            ".with_srtp(",
            call_source,
            "nothing in sipx-call selects SRTP any more, so RFC 3711 and 4568 are now the shape"
            " this check was written to catch — their roles are no longer supportable",
        )
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (3711, 4568):
            with self.subTest(rfc=number):
                self.assertEqual(["uac", "uas"], by_number[number]["roles"])

        # RFC 8445 and 8839 claim nothing: ICE is selected through `start_with_ice`, and the crate
        # that serves the role does not mention ICE at all — not the session, not the candidates,
        # not the gathering. Word-boundary matched, because `alice` is all over these fixtures.
        self.assertIsNone(
            re.search(r"\bice\b", call_source, re.I),
            "sipx-call now mentions ICE. If a call can select it, RFC 8445 and 8839 may claim"
            " roles again and `M-27` is done — update those rows and this test. If it is only a"
            " comment, this assertion is what needs relaxing, not the rows.",
        )
        self.assertNotIn("start_with_ice", call_source)

        # RFC 8122 claims nothing for the subtler reason: `sipx-call` *does* render
        # `a=fingerprint`, so a path-based check could be satisfied by citing it — but the branch
        # is guarded by a capability nothing outside `sipx-sdp`'s unit tests ever sets. This is the
        # dead-branch limit of the check, recorded in the design as what would widen it.
        self.assertIn(
            "fingerprint",
            call_source,
            "the dead `a=fingerprint` branch is gone; if DTLS-SRTP was wired rather than deleted,"
            " RFC 8122 can claim roles again",
        )
        self.assertNotIn("with_dtls_srtp", call_source)

    def test_only_a_crate_path_proves_reachability(self):
        """The repository-root `tests/` tree is not a way through the check.

        `evidence` may legitimately cite markdown — RFC 5922 cites a spec — so accepting the root
        tree wholesale would have let `tests/interop/README.md` stand as proof that a role is
        reachable. Its Rust half is in `crates/sipx-cli/tests/`, which is accepted on its own.
        """
        entry = self.a_media_entry(evidence=["tests/interop/README.md"])
        self.assertTrue(
            any("reach" in p for p in report.check([entry])),
            "a path outside crates/ was taken as proof a call can reach the capability",
        )

    def test_the_call_layer_is_read_from_the_workspace(self):
        """The reachable set is Cargo's dependency graph, not a list in this script.

        A hand-kept list is the failure `gate.py` exists to prevent one directory over: a new
        crate above `sipx-call` would silently fail to count as reachable.
        """
        crates = report.call_layer_crates()
        self.assertIn("sipx-call", crates)
        self.assertIn("sipx-cli", crates)
        self.assertIn("sipx-app-protocol", crates)
        # Below sipx-call, so citing them proves nothing about what a call can select.
        self.assertNotIn("sipx-media", crates)
        self.assertNotIn("sipx-sdp", crates)

    def test_the_media_rows_that_prompted_this_story_claim_no_unreachable_role(self):
        """RFC 8122, 8445 and 8839, named in X-30's Notes as carrying the shape today.

        Asserted by number rather than left to the registry-wide check, so that restoring a role
        to one of them names the story that removed it.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (5763, 5764, 8122, 8445, 8839):
            with self.subTest(rfc=number):
                self.assertEqual([], by_number[number].get("roles", []))


class TheRealRegistry(unittest.TestCase):
    """The guard is only worth having if the registry it guards already satisfies it."""

    def test_every_entry_satisfies_the_schema(self):
        for entry in registry_entries():
            with self.subTest(rfc=entry.get("number")):
                self.assertEqual([], report.schema_problems(entry))

    def test_the_registry_has_no_outstanding_problems(self):
        self.assertEqual([], report.check(registry_entries()))

    def test_rfc_number_is_a_usable_reference_key(self):
        """A downstream inherits a kernel row by its RFC number, so the number must identify one.

        `docs/rfc/README.md` promises exactly this to a downstream registry pinning a kernel
        version. Uniqueness is what makes `inherits = 3261` resolve to a single claim.
        """
        numbers = [e["number"] for e in registry_entries()]
        self.assertEqual(len(numbers), len(set(numbers)))
        for number in numbers:
            self.assertIsInstance(number, int)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
