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

        The scope is a choice. Media is where the crate serving a role and the crate implementing
        the capability come apart — a media row claims `uac`/`uas`, which an application reaches
        through `sipx-call`, while the code sits in `sipx-media` or `sipx-sdp`, and twice nothing
        connected the two. A security row like RFC 2617 is reachable through `sipx-cli`, which
        depends on `sipx-ua` *and* `sipx-call`; it is out of scope because the check would be
        asking a question that cannot come out `no`, not because it could not answer it.
        `docs/designs/rfc-registry-grain.md` carries the count and the argument.
        """
        entry = self.a_media_entry(
            layer="security", evidence=["crates/sipx-ua/src/auth.rs"]
        )
        self.assertEqual([], [p for p in report.check([entry]) if "reach" in p])

    def test_the_services_rows_keep_their_roles_and_why(self):
        """RFC 3680, 3856, 3903 and 4235 are *not* the media over-claims one layer over.

        They implement a `uas` surface in `sipx-ua` that nothing in `sipx-cli` calls, which looks
        like the ICE shape and is not: sipx is a library, and `sipx-ua` is itself the API an
        application serves subscriptions and publications through. `crates/sipx-ua/tests/
        packages.rs` imports `sipx_ua::presence` and `sipx_ua::packages` across the crate
        boundary, so an external consumer demonstrably reaches them — which is precisely what
        `Capabilities::with_dtls_srtp` and `MediaSession::start_with_ice` have no example of.

        Asserted so that the distinction is a recorded judgement rather than an omission, and so
        that removing that integration test surfaces here.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (3680, 3856, 3903, 4235):
            with self.subTest(rfc=number):
                self.assertTrue(by_number[number].get("roles"))
        reached = (ROOT / "crates" / "sipx-ua" / "tests" / "packages.rs").read_text()
        self.assertIn("sipx_ua::presence", reached)
        self.assertIn("sipx_ua::packages", reached)

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
