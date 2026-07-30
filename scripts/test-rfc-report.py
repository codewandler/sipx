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


class ClaimReachability(unittest.TestCase):
    """A row at a *selection* layer may not claim what no call can select.

    The registry's one failure mode, five times in two days: a capability is built in a crate,
    the row claims `uac` and `uas`, and nothing above the crate ever calls it — so the compliance
    table reports as shipped something an application cannot ask for. Every existing check passes
    for such a row, because the header is known, the file exists, and evidence was cited.

    `sipx-call` is where an application asks for a call, so a capability whose evidence never
    reaches it is one no role can perform. `X-30` scoped that to `layer = "media"`; `X-33` widened
    it along both axes it was measured to be narrow on — the layer (`security` selects credentials
    and a secure transport exactly as media selects a keying) and the field (`status =
    "implemented"` is a claim in the same table as `roles`). See
    `docs/designs/rfc-registry-grain.md` for the measurement behind each.
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

    def test_a_security_role_claimed_from_a_leaf_crate_is_rejected(self):
        """`X-33`'s failing-first case: the same shape as ICE, one layer over.

        Security is a selection layer for the same reason media is. Credentials are *supplied* —
        `crates/sipx-cli/src/register.rs:95` is the only `with_credentials` above the call layer,
        and it is inside an `if let Some(password)`, so supplying nothing is the silent default and
        the REGISTER still succeeds unchallenged. A secure transport is *chosen* the same way: a
        `Target` carries its `TransportKind`, and choosing UDP is the default.

        Before `X-33` this row passed `--check`, because `ROLE_REACHABILITY_LAYERS` was `{"media"}`
        — and relabelling a media row `security` was therefore the documented way out of the check.
        This assertion is the same fixture the old `test_the_rule_is_scoped_to_the_media_layer`
        used to prove the escape worked.
        """
        entry = self.a_media_entry(
            layer="security", evidence=["crates/sipx-ua/src/auth.rs"]
        )
        problems = report.check([entry])
        self.assertTrue(
            any("9999" in p and "reach" in p for p in problems),
            f"a security role no call can select was accepted; problems={problems}",
        )

    def test_an_implemented_media_row_with_no_role_must_still_reach_a_call(self):
        """The `roles`-not-`status` hole `X-30` recorded, in fixture form: RFC 6716 and 7587.

        Both were `layer = "media"`, `status = "implemented"`, and carried no `roles` at all, so
        `unreachable_role_claims` returned before asking anything. Opus is unreachable on four
        independent grounds — no `with_opus` caller outside `sipx-sdp`'s own tests,
        `Codec::from_payload_type` deliberately never returns it, no `sipx-call` entry point takes
        caller-supplied `Capabilities`, and the `opus` feature is off at every level — yet the
        generated table said "✅ implemented" under a heading that reads "What sipx implements".

        A row with no role claims nothing about a role. It still claims a status, in the same
        table, and at a selection layer that claim is the same over-claim one field over.
        """
        entry = self.a_media_entry(
            roles=[], evidence=["crates/sipx-audio/src/opus.rs"]
        )
        problems = report.check([entry])
        self.assertTrue(
            any("9999" in p and "reach" in p for p in problems),
            f"an unreachable `implemented` media row was accepted; problems={problems}",
        )

    def test_the_status_gate_is_media_only_and_the_reason_is_measured(self):
        """The limit of the status half, asserted rather than only written down.

        `implemented` is held to reachability at the media layer and not at the security layer, and
        the reason is a measurement rather than a preference: of the seven `implemented` security
        rows, the three carrying no role — 6125, 8446 and 8996 — state *policies* of the TLS stack
        (a non-matching SAN is refused rather than falling back to the CN; 1.3 preferred; 1.2 is the
        floor and not configurable downward). A policy holds on every connection and is proved by
        the *absence* of an API, so "which call reaches it" is not the question those rows answer.
        Every media `implemented` row, by contrast, names a capability a call either carries or does
        not.

        **This test is also the remaining dodge**, and is written here so it is visible in the
        check's own tests: an unreachable media row relabelled `security` escapes the status half.
        What closes it for the rows that would want it is `misdeclared_layer` — see
        `test_a_media_crate_citation_pins_the_layer`.
        """
        entry = self.a_media_entry(
            layer="security", roles=[], evidence=["crates/sipx-transport/src/tls.rs"]
        )
        self.assertEqual([], [p for p in report.check([entry]) if "reach" in p])

    def test_a_media_crate_citation_pins_the_layer(self):
        """`layer` is author-set, so the crates that are only ever media pin it.

        `X-30` recorded relabelling as the strongest argument against scoping by layer at all.
        This closes it for the class of row that would want the dodge: `sipx-media`, `sipx-rtp` and
        `sipx-audio` have no subject other than media, so a row citing one of them is a media row
        whatever its `layer` says. To leave the check an author would now have to stop citing their
        own implementation — and `evidence` existence is already checked.

        The three crates are the exact set: no non-media row in the registry cites any of them
        today, which `test_no_row_declares_a_layer_its_evidence_contradicts` holds.
        """
        entry = self.a_media_entry(
            layer="security", evidence=["crates/sipx-media/src/ice/agent.rs"]
        )
        problems = report.check([entry])
        self.assertTrue(
            any("9999" in p and "layer" in p for p in problems),
            f"a media-crate row was allowed to declare another layer; problems={problems}",
        )

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

    def test_the_services_rows_keep_their_roles_only_while_nothing_dispatches_to_them(self):
        """`X-33` resolving the four rows individually, and pinning the reason so it can expire.

        `X-30` gave these rows one collective argument. Taken row by row the argument is the same
        one four times, and it holds — but only because of a fact nobody had checked: **no crate in
        this workspace receives a SUBSCRIBE or a PUBLISH off a socket.** `Subscriptions::on_subscribe`
        and `Compositor::apply` take a parsed `Request` and are fed one by `sipx-ua`'s own tests;
        `sipx-call`'s dispatcher advertises neither method on `Allow` and unit-tests that it does not.

        That is what makes `sipx-ua` the crate that *serves* the role rather than a crate below one
        that must select the capability — there is no `sipx-call` for subscriptions, so asking these
        rows to cite one would ask them to cite a crate that does not and should not depend on them.
        The four resolutions, individually:

        - **3903** (PUBLISH, `uas`): `Compositor::apply` decides what a publication means and what to
          answer; an application supplies the request. Role kept, note says so.
        - **3856** (presence package, `uas`): joined to the notifier and driven from outside the
          crate — `packages.rs` publishes and asserts the NOTIFY body changes. Role kept.
        - **3680** (`reg` package, `uas`): registered under the name a subscriber asks for, asserted
          from outside the crate. Role kept; the missing registrar join was already in the note.
        - **4235** (`dialog` package, `uas`): same, plus the missing dialog-store join. Role kept.

        **And the trigger that takes the roles away is this assertion.** The moment something routes
        an inbound SUBSCRIBE or PUBLISH — a dispatcher, a server mode, an application host — these
        rows acquire the media shape exactly, because then there *is* a crate above `sipx-ua` that
        must select the package, and a package nothing selects is unreachable. This test goes red
        first, which is the point of writing it here instead of in prose.
        """
        dispatch = (ROOT / "crates" / "sipx-call" / "src" / "dispatch.rs").read_text()
        served = re.findall(r"Method::(\w+)\s*(?:=>|\|)", dispatch)
        # Non-vacuous: the dispatcher does route six methods, `Notify` among them — which is how
        # RFC 6665's own `uas` claim reaches a call, through REFER's implicit subscription.
        self.assertIn("Notify", served)
        for method in ("Subscribe", "Publish"):
            with self.subTest(method=method):
                self.assertNotIn(
                    method,
                    served,
                    f"something now dispatches on {method}. If an inbound one reaches a package,"
                    " RFC 3680, 3856, 3903 and 4235 are the media over-claim shape one layer over"
                    " and their `uas` claims need a caller above `sipx-ua` — re-read"
                    " docs/designs/rfc-registry-grain.md before relaxing this",
                )

        # And no request-routing anywhere in the crate that serves them either. `sipx-ua` is a
        # library: it decides what a SUBSCRIBE means and never learns one arrived.
        for source in (ROOT / "crates" / "sipx-ua" / "src").rglob("*.rs"):
            text = source.read_text()
            for method in ("Method::Subscribe", "Method::Publish"):
                with self.subTest(source=source.name, method=method):
                    self.assertNotIn(method, text)

        # Each of the four resolutions is a role that is still claimed, so a silent removal is a
        # change to the argument above and not just to a row.
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (3680, 3856, 3903, 4235):
            with self.subTest(rfc=number):
                self.assertEqual(["uas"], by_number[number]["roles"])

    def test_the_scope_tracks_selection_not_the_layer_string(self):
        """The property behind `ROLE_REACHABILITY_LAYERS`, held against the code.

        A media capability is *selected*: it is carried only because something asked for it, and
        asking for nothing is the silent default. That — not the layer string — is why media rows
        are the ones checked. `layer` is a proxy, so this test holds the two in agreement on the
        media rows that claim a role, and a registry where they part company fails the gate instead
        of reading as measured.

        Scope of the agreement: `X-33` closed the limit this docstring used to record. A row without
        `roles` was outside the check entirely, so RFC 6716 and 7587 claimed `implemented` for Opus,
        which no call can select, and nothing objected. `status = "implemented"` is now held to
        reachability at this layer too, and both rows are `partial` — see
        `test_an_implemented_media_row_with_no_role_must_still_reach_a_call`.

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

    def test_a_non_source_file_in_a_call_layer_crate_proves_nothing(self):
        """The same hole one directory in, which `X-30` recorded and left open.

        `crates/sipx-call/README.md` would have satisfied a rule that only asked which crate the
        path is in. Closing it costs one condition and nothing else: of the registry's 117 evidence
        paths exactly two are not `.rs` files, both `docs/specs/sip-tls.md` (RFC 5922's and 8996's),
        and both are outside `crates/`. (The design said "80 paths, exactly one" — measured here
        rather than inherited, and both halves of that were stale.)
        """
        entry = self.a_media_entry(evidence=["crates/sipx-cli/Cargo.toml"])
        self.assertTrue(
            any("reach" in p for p in report.check([entry])),
            "a non-source file in a call-layer crate was taken as proof of reachability",
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

    def test_the_security_scope_tracks_selection_too(self):
        """The property behind adding `security`, held against the code rather than asserted.

        Media earned its place in the scope because a capability there is *selected* and selecting
        nothing is the silent default. Security has the same two halves, and both are checked here,
        because the layer string is a proxy in exactly the way it is for media:

        - **credentials are selected**: `Config::with_credentials` is the opt-in, and the only
          caller above the call layer is `crates/sipx-cli/src/register.rs`;
        - **and omitting them is silent**: that call is inside an `if let Some(password)`, so
          `sipx register` with no password still registers, and nothing fails.

        The transport half is the same shape — a `Target` carries its `TransportKind`, and the
        secure one is reached only because a caller asked for it: `crates/sipx-call/tests/wss.rs`
        chooses `TransportKind::Wss` and verifies the certificate against the URI host, which is
        RFC 5922's whole claim.
        """
        register = (ROOT / "crates" / "sipx-cli" / "src" / "register.rs").read_text()
        self.assertIn(".with_credentials(", register)
        self.assertIn("if let Some(password)", register)

        cli_sources = (ROOT / "crates" / "sipx-cli" / "src").rglob("*.rs")
        callers = [
            path.name
            for path in cli_sources
            if ".with_credentials(" in path.read_text()
        ]
        self.assertEqual(
            ["register.rs"],
            callers,
            "the set of credential selectors above the call layer changed; RFC 2617, 7616 and 8760"
            " cite register.rs as the caller that makes their `uac` claim reachable",
        )

        wss = (ROOT / "crates" / "sipx-call" / "tests" / "wss.rs").read_text()
        self.assertIn("TransportKind::Wss", wss)
        self.assertIn(".verifying(", wss)
        self.assertIn("TrustAnchors::only()", wss)

    def test_the_security_rows_cite_what_selects_them(self):
        """The four security rows that claim a role, resolved individually rather than in a batch.

        `X-30`'s first justification for stopping at media was that seven `sipx-ua` rows "cannot
        satisfy the rule at any price". They can: `sipx-cli` depends on both `sipx-ua` and
        `sipx-call` (`crates/sipx-cli/Cargo.toml:21-22`), so it is in `call_layer_crates()` and its
        files are admissible evidence. Three of these rows needed one honest citation each; the
        fourth needed a different one.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (2617, 7616, 8760):
            with self.subTest(rfc=number):
                self.assertIn(
                    "crates/sipx-cli/src/register.rs",
                    by_number[number]["evidence"],
                    "digest is selected by supplying credentials, and register.rs is the only"
                    " caller above the call layer that does",
                )
        self.assertIn(
            "crates/sipx-call/tests/wss.rs",
            by_number[5922]["evidence"],
            "the identity check is reached only by a call that chose a secure transport",
        )

    def test_the_opus_rows_no_longer_claim_more_than_a_call_can_ask_for(self):
        """`X-33`'s registry half: the two rows the widened field axis rejects.

        Demoted rather than explained. There is no suppression list, so a row that cannot be made
        true changes what the published table says — `partial` plus a note naming the gap, which is
        the form RFC 5763, 5764, 8122, 8445 and 8839 already use for the same fact.
        """
        by_number = {e["number"]: e for e in registry_entries()}
        for number in (6716, 7587):
            with self.subTest(rfc=number):
                self.assertEqual("partial", by_number[number]["status"])
                self.assertIn("Missing: a call", by_number[number]["note"])

        # And the reason, still true: nothing above `sipx-sdp`'s own tests asks for Opus.
        for crate in ("sipx-call", "sipx-cli"):
            sources = (ROOT / "crates" / crate / "src").rglob("*.rs")
            for path in sources:
                with self.subTest(path=path.name):
                    self.assertNotIn("with_opus", path.read_text())

    def test_no_row_declares_a_layer_its_evidence_contradicts(self):
        """The registry satisfies the layer-consistency rule the check now enforces."""
        for entry in registry_entries():
            with self.subTest(rfc=entry["number"]):
                self.assertEqual([], report.misdeclared_layer(entry))

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
